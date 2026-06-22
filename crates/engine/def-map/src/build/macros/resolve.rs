//! Resolves macro-call paths against the current def-map scope snapshot.
//!
//! Macro resolution mostly reuses normal path resolution, but single-segment calls need direct
//! macro-namespace lookup so locally imported macros can shadow builtin names.

use anyhow::Result;

use rg_ir_model::{DefId, DefMapRef, LocalDefRef, ModuleRef, PathSegment, TargetRef};
use rg_ir_storage::{
    ImportPath, LocalDefData, MacroDefinitionData, MacroDefinitionEnv, PathResolver, ScopeBinding,
    ScopeBindingOrigin, TargetResolutionEnv,
};
use rg_std::ExpectedUnique;
use rg_text::Name;

use crate::build::{collect::TargetState, finalize::FinalizeTargetStates};

use super::{ItemOrder, MacroCallSite};

/// Macro definition resolved through the ordinary macro namespace.
pub(super) struct ResolvedMacroDefinition<'a> {
    pub(super) def_ref: LocalDefRef,
    pub(super) local_def: &'a LocalDefData,
    pub(super) data: &'a MacroDefinitionData,
    pub(super) order: Option<&'a ItemOrder>,
    pub(super) origin: ScopeBindingOrigin,
}

impl PartialEq for ResolvedMacroDefinition<'_> {
    fn eq(&self, other: &Self) -> bool {
        // Collapse duplicate bindings to the same macro definition, e.g. macro-export root aliases.
        self.def_ref == other.def_ref
    }
}

impl Eq for ResolvedMacroDefinition<'_> {}

/// Applies item-position macro lookup rules on top of ordinary path resolution.
///
/// Macro calls are mostly path-shaped, but one-segment calls have extra Rust-specific behavior:
/// textual `macro_rules!` definitions can shadow namespace bindings, legacy `#[macro_use]` imports
/// are a fallback, and unresolved names may still be known builtins that def-map should classify
/// rather than retry forever. This resolver keeps that policy together while reusing
/// `PathResolver` for the ordinary namespace work.
pub(super) struct ItemMacroResolver<'a, E: ?Sized> {
    env: &'a E,
    states: &'a FinalizeTargetStates,
    state: &'a TargetState,
}

impl<'a, E> ItemMacroResolver<'a, E>
where
    E: TargetResolutionEnv<Error = rg_package_store::PackageStoreError>
        + MacroDefinitionEnv
        + ?Sized,
{
    pub(super) fn new(
        env: &'a E,
        states: &'a FinalizeTargetStates,
        state: &'a TargetState,
    ) -> Self {
        Self { env, states, state }
    }

    /// Finds the unique declarative macro definition visible at a macro call.
    pub(super) fn resolve(
        &self,
        call: &MacroCallSite,
        path: &ImportPath,
    ) -> Result<Option<ResolvedMacroDefinition<'a>>> {
        if let Some(name) = path.relative_single_name() {
            // Unqualified calls have special `macro_rules!` textual visibility before they behave
            // like ordinary macro-namespace lookups.
            return self.resolve_single_name_macro(call, name);
        }

        // Qualified calls follow ordinary path resolution for the prefix, then keep the final macro
        // binding so order filtering can distinguish direct definitions from exports/imports.
        let resolved_bindings =
            PathResolver::new(self.env).macro_bindings(self.state.target, call.module, path)?;
        let mut macros = Vec::new();

        for binding in resolved_bindings {
            if let Some(resolved) = self.macro_record_for_binding(&binding)? {
                macros.push(resolved);
            }
        }

        Ok(
            unique_macro_definition(visible_macro_definitions(macros, self.state.target, call))
                .into_option(),
        )
    }

    /// Resolves one-segment macro calls with Rust's `macro_rules!` lookup order.
    fn resolve_single_name_macro(
        &self,
        call: &MacroCallSite,
        name: &Name,
    ) -> Result<Option<ResolvedMacroDefinition<'a>>> {
        // Textual `macro_rules!` scope shadows ordinary macro bindings. This covers the
        // source-order behavior that cannot be represented by a normal module-scope map.
        if let Some(resolved) = self.resolve_textual_macro_rules(call, name)? {
            return Ok(Some(resolved));
        }

        // Imported/reexported macros and exported root macros are represented as ordinary macro
        // namespace bindings in the current resolved scope snapshot.
        let entry = self.env.module_scope_entry(
            ModuleRef {
                origin: DefMapRef::Target(self.state.target),
                module: call.module,
            },
            name.as_str(),
        )?;

        if let Some(entry) = entry {
            let mut macros = Vec::new();
            for binding in entry.macros() {
                if let Some(resolved) = self.macro_record_for_binding(binding)? {
                    macros.push(resolved);
                }
            }

            match unique_macro_definition(visible_macro_definitions(
                macros,
                self.state.target,
                call,
            )) {
                ExpectedUnique::Empty => {}
                ExpectedUnique::One(macro_) => return Ok(Some(macro_)),
                ExpectedUnique::Ambiguous => return Ok(None),
            }
        }

        self.resolve_macro_use_extern_crate_fallback(name)
    }

    /// Searches build-only textual `macro_rules!` scopes for the definition visible at this call.
    fn resolve_textual_macro_rules(
        &self,
        call: &MacroCallSite,
        name: &Name,
    ) -> Result<Option<ResolvedMacroDefinition<'a>>> {
        let mut module = call.module;
        let mut boundary = &call.order;

        loop {
            // In the current module, use the latest declaration that appears before the call. When
            // we climb to a parent, `boundary` becomes the child module declaration instead.
            if let Some(local_def) = self
                .state
                .textual_macro_scopes
                .latest_before(module, name, boundary)
            {
                return self.macro_record_for_def(
                    DefId::Local(LocalDefRef {
                        origin: DefMapRef::Target(self.state.target),
                        local_def,
                    }),
                    ScopeBindingOrigin::Direct,
                );
            }

            // A parent module contributes only declarations that appeared before the child module
            // was declared, matching the textual file view used by `macro_rules!`.
            let Some(parent) = self.env.parent_module(ModuleRef {
                origin: DefMapRef::Target(self.state.target),
                module,
            })?
            else {
                return Ok(None);
            };
            let Some(parent_boundary) = self
                .state
                .textual_macro_scopes
                .module_declaration_order(module)
            else {
                return Ok(None);
            };

            boundary = parent_boundary;
            module = parent.module;
        }
    }

    /// Consults legacy `#[macro_use] extern crate` imports after ordinary unqualified lookup fails.
    fn resolve_macro_use_extern_crate_fallback(
        &self,
        name: &Name,
    ) -> Result<Option<ResolvedMacroDefinition<'a>>> {
        let mut macros = Vec::new();

        for macro_use in &self.state.macro_use_imports {
            if !macro_use.selector.allows(name) {
                continue;
            }

            let import_owner = ModuleRef {
                origin: DefMapRef::Target(self.state.target),
                module: macro_use.module,
            };
            for binding in PathResolver::new(self.env).visible_macro_bindings(
                import_owner,
                macro_use.source_module,
                name,
            )? {
                // Treat the fallback as an import-like binding. The source binding may be direct
                // inside the exporting crate, but macro-use lookup is not source-order sensitive in
                // the caller.
                if let Some(resolved) =
                    self.macro_record_for_def(binding.def, ScopeBindingOrigin::Import)?
                {
                    macros.push(resolved);
                }
            }
        }

        Ok(unique_macro_definition(macros).into_option())
    }

    /// Converts a resolved macro binding into the payload needed by expansion.
    fn macro_record_for_binding(
        &self,
        binding: &ScopeBinding,
    ) -> Result<Option<ResolvedMacroDefinition<'a>>> {
        self.macro_record_for_def(binding.def, binding.origin)
    }

    /// Converts a resolved definition id into the macro payload needed by expansion.
    fn macro_record_for_def(
        &self,
        def: DefId,
        origin: ScopeBindingOrigin,
    ) -> Result<Option<ResolvedMacroDefinition<'a>>> {
        let Some(payload) = MacroDefinitionEnv::macro_definition_view(self.env, def)? else {
            return Ok(None);
        };
        let order = payload
            .def_ref
            .origin
            .as_target_ref()
            .and_then(|target| self.states.target(target))
            .and_then(|state| state.macro_definitions.get(&payload.def_ref.local_def))
            .map(|record| &record.order);

        Ok(Some(ResolvedMacroDefinition {
            def_ref: payload.def_ref,
            local_def: payload.local_def,
            data: payload.data,
            order,
            origin,
        }))
    }
}

fn visible_macro_definitions<'a, 'call, I>(
    macros: I,
    target: TargetRef,
    call: &'call MacroCallSite,
) -> impl Iterator<Item = ResolvedMacroDefinition<'a>> + 'call
where
    I: IntoIterator<Item = ResolvedMacroDefinition<'a>> + 'call,
{
    macros
        .into_iter()
        .filter(move |macro_| macro_definition_is_visible_by_order(macro_, target, call))
}

fn unique_macro_definition<'a>(
    macros: impl IntoIterator<Item = ResolvedMacroDefinition<'a>>,
) -> ExpectedUnique<ResolvedMacroDefinition<'a>> {
    let mut unique = ExpectedUnique::new();
    for macro_ in macros {
        // A root `#[macro_export]` macro can appear as both its ordinary definition and exported
        // root binding. That is still one resolved macro, not an ambiguity.
        unique.push(macro_);
    }

    unique
}

/// Filters ordinary namespace candidates that are textually later than the call site.
fn macro_definition_is_visible_by_order(
    macro_: &ResolvedMacroDefinition<'_>,
    target: TargetRef,
    call: &MacroCallSite,
) -> bool {
    if macro_.origin != ScopeBindingOrigin::Direct {
        return true;
    }

    !(macro_.def_ref.origin == DefMapRef::Target(target)
        && macro_.local_def.module == call.module
        && macro_.order.is_some_and(|order| order > &call.order))
}

/// Known builtin macro call that should not be resolved as a user macro.
pub(super) enum BuiltinMacroDisposition {
    /// The builtin cannot add module-scope definitions, so def-map can safely ignore it.
    IgnoredByDefMap,
    /// The builtin selects one item stream from cfg predicates.
    CfgSelect,
    /// The builtin splices another source file into the caller's item stream.
    Include,
    /// The builtin can affect item collection or requires dedicated compiler-like handling.
    Unsupported,
}

impl BuiltinMacroDisposition {
    /// Classifies builtin macros that are known even when no user macro binding resolves.
    ///
    /// We intentionally take a small shortcut here. Unqualified builtins and `std`/`core`-qualified
    /// builtin-shaped paths cover the realistic call sites, while a fully resolution-aware builtin
    /// prefix model would add a lot of complexity for rare local `std`/`core` shadowing cases.
    pub(super) fn from_path(path: &ImportPath) -> Option<Self> {
        let name = path.relative_single_name().or_else(|| {
            // Check if we have two segments and the first one is either `std` or `core`.
            let [PathSegment::Name(root), PathSegment::Name(name)] = path.segments.as_slice()
            else {
                return None;
            };
            matches!(root.as_str(), "std" | "core").then_some(name)
        })?;

        match name.as_str() {
            // Expression, diagnostic, or assembly builtins do not contribute named items to def-map.
            // Body lowering can later synthesize values/types for the expression-like subset.
            "asm" | "cfg" | "column" | "compile_error" | "concat" | "env" | "file"
            | "format_args" | "global_asm" | "include_bytes" | "include_str" | "line"
            | "llvm_asm" | "module_path" | "option_env" | "stringify" => {
                Some(Self::IgnoredByDefMap)
            }

            "cfg_select" => Some(Self::CfgSelect),
            "include" => Some(Self::Include),

            // `concat_idents!` has token-shaping behavior that is better handled by a dedicated
            // builtin implementation.
            "concat_idents" => Some(Self::Unsupported),
            _ => None,
        }
    }
}
