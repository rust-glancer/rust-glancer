//! Resolves macro-call paths against the current def-map scope snapshot.
//!
//! Macro resolution mostly reuses normal path resolution, but single-segment calls need direct
//! macro-namespace lookup so locally imported macros can shadow builtin names.

use anyhow::Result;

use rg_ir_model::{DefId, DefMapRef, LocalDefRef, ModuleRef, TargetRef};
use rg_ir_storage::{
    ImportPath, LocalDefData, MacroDefinitionData, MacroDefinitionEnv, ScopeResolver, ScopeBinding,
    ScopeBindingOrigin, TargetResolutionEnv,
};
use rg_std::ExpectedUnique;
use rg_text::Name;

use super::{ItemOrder, MacroCallSite};
use crate::build::{collect::TargetState, finalize::FinalizeTargetStates};

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
/// are a fallback, and resolved compiler builtin definitions need to carry their builtin identity
/// into expansion dispatch. This resolver keeps that policy together while reusing `ScopeResolver`
/// for the ordinary namespace work.
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
    ) -> Result<ExpectedUnique<ResolvedMacroDefinition<'a>>> {
        if let Some(name) = path.relative_single_name() {
            // Unqualified calls have special `macro_rules!` textual visibility before they behave
            // like ordinary macro-namespace lookups.
            return self.resolve_single_name_macro(call, name);
        }

        // Qualified calls follow ordinary path resolution for the prefix, then keep the final macro
        // binding so order filtering can distinguish direct definitions from exports/imports.
        let resolved_bindings = ScopeResolver::new(self.env).macro_bindings(
            ModuleRef::target(self.state.target, call.module),
            path,
        )?;
        let mut macros = Vec::new();

        for binding in resolved_bindings {
            if let Some(resolved) = self.macro_record_for_binding(&binding)? {
                macros.push(resolved);
            }
        }

        Ok(unique_macro_definition(visible_macro_definitions(
            macros,
            self.state.target,
            call,
        )))
    }

    /// Resolves one-segment macro calls with Rust's `macro_rules!` lookup order.
    fn resolve_single_name_macro(
        &self,
        call: &MacroCallSite,
        name: &Name,
    ) -> Result<ExpectedUnique<ResolvedMacroDefinition<'a>>> {
        // Textual `macro_rules!` scope shadows ordinary macro bindings. This covers the
        // source-order behavior that cannot be represented by a normal module-scope map.
        if let Some(resolved) = self.resolve_textual_macro_rules(call, name)? {
            return Ok(ExpectedUnique::One(resolved));
        }

        match self.resolve_scope_macro(call, name)? {
            ExpectedUnique::One(resolved) => return Ok(ExpectedUnique::One(resolved)),
            ExpectedUnique::Ambiguous => return Ok(ExpectedUnique::Ambiguous),
            ExpectedUnique::Empty => {}
        }

        self.resolve_macro_use_extern_crate_fallback(name)
    }

    /// Resolves macros from the current module or selected standard prelude.
    fn resolve_scope_macro(
        &self,
        call: &MacroCallSite,
        name: &Name,
    ) -> Result<ExpectedUnique<ResolvedMacroDefinition<'a>>> {
        let importing_module = ModuleRef {
            origin: DefMapRef::Target(self.state.target),
            module: call.module,
        };
        let bindings = ScopeResolver::new(self.env).visible_unqualified_macro_bindings(
            importing_module,
            [importing_module],
            name,
        )?;

        let module_scope = self.resolve_scope_macro_bindings(call, bindings.module_scope)?;
        if !module_scope.is_empty() {
            return Ok(module_scope);
        }

        self.resolve_scope_macro_bindings(call, bindings.standard_prelude)
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
    ) -> Result<ExpectedUnique<ResolvedMacroDefinition<'a>>> {
        let mut macros = Vec::new();

        for macro_use in &self.state.macro_use_imports {
            if !macro_use.selector.allows(name) {
                continue;
            }

            let import_owner = ModuleRef {
                origin: DefMapRef::Target(self.state.target),
                module: macro_use.module,
            };
            for binding in ScopeResolver::new(self.env).visible_macro_bindings(
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

        Ok(unique_macro_definition(macros))
    }

    fn resolve_scope_macro_bindings(
        &self,
        call: &MacroCallSite,
        bindings: Vec<ScopeBinding>,
    ) -> Result<ExpectedUnique<ResolvedMacroDefinition<'a>>> {
        let mut macros = Vec::new();

        for binding in bindings {
            if let Some(resolved) = self.macro_record_for_binding(&binding)? {
                macros.push(resolved);
            }
        }

        Ok(unique_macro_definition(visible_macro_definitions(
            macros,
            self.state.target,
            call,
        )))
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
