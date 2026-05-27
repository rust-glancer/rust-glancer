//! Resolves macro-call paths against the current def-map scope snapshot.
//!
//! Macro resolution mostly reuses normal path resolution, but single-segment calls need direct
//! macro-namespace lookup so locally imported macros can shadow builtin names.

use anyhow::Result;

use rg_ir_model::{DefId, LocalDefRef, ModuleRef, TargetRef};
use rg_text::Name;

use crate::{
    ImportPath, LocalDefData, LocalDefKind, MacroDefinitionData, PathSegment, ScopeBinding,
    ScopeBindingOrigin,
    build::{collect::TargetState, finalize::FinalizeTargetStates},
    query::path_resolution::{
        PathResolutionEnv, resolve_path_to_macro_bindings_with_env,
        visible_module_macro_bindings_with_env,
    },
};

use super::{ItemOrder, MacroCallSite};

/// Macro definition resolved through the ordinary macro namespace.
pub(super) struct ResolvedMacroDefinition<'a> {
    pub(super) def_ref: LocalDefRef,
    pub(super) local_def: &'a LocalDefData,
    pub(super) data: &'a MacroDefinitionData,
    pub(super) order: Option<&'a ItemOrder>,
    pub(super) origin: ScopeBindingOrigin,
}

/// Finds the unique declarative macro definition visible at a macro call.
pub(super) fn resolve_macro_definition<'a>(
    env: &'a impl PathResolutionEnv,
    states: &'a FinalizeTargetStates,
    state: &TargetState,
    call: &MacroCallSite,
    path: &ImportPath,
) -> Result<Option<ResolvedMacroDefinition<'a>>> {
    if let Some(name) = relative_single_name(path) {
        // Unqualified calls have special `macro_rules!` textual visibility before they behave like
        // ordinary macro-namespace lookups.
        return resolve_single_name_macro(env, states, state, call, name);
    }

    // Qualified calls follow ordinary path resolution for the prefix, then keep the final macro
    // binding so order filtering can distinguish direct definitions from exports/imports.
    let resolved_bindings =
        resolve_path_to_macro_bindings_with_env(env, state.target, call.module, path)?;
    let mut macros = Vec::new();

    for binding in resolved_bindings {
        if let Some(resolved) = macro_record_for_binding(env, states, &binding)? {
            macros.push(resolved);
        }
    }

    Ok(
        unique_macro_definition(visible_macro_definitions(macros, state.target, call))
            .into_option(),
    )
}

/// Resolves one-segment macro calls with Rust's `macro_rules!` lookup order.
fn resolve_single_name_macro<'a>(
    env: &'a impl PathResolutionEnv,
    states: &'a FinalizeTargetStates,
    state: &TargetState,
    call: &MacroCallSite,
    name: &Name,
) -> Result<Option<ResolvedMacroDefinition<'a>>> {
    // Textual `macro_rules!` scope shadows ordinary macro bindings. This covers the source-order
    // behavior that cannot be represented by a normal module-scope map.
    if let Some(resolved) = resolve_textual_macro_rules(env, states, state, call, name)? {
        return Ok(Some(resolved));
    }

    // Imported/reexported macros and exported root macros are represented as ordinary macro
    // namespace bindings in the current resolved scope snapshot.
    let entry = env.module_scope_entry(
        ModuleRef {
            target: state.target,
            module: call.module,
        },
        name.as_str(),
    )?;

    if let Some(entry) = entry {
        let mut macros = Vec::new();
        for binding in entry.macros() {
            if let Some(resolved) = macro_record_for_binding(env, states, binding)? {
                macros.push(resolved);
            }
        }

        match unique_macro_definition(visible_macro_definitions(macros, state.target, call)) {
            MacroDefinitionSelection::Empty => {}
            MacroDefinitionSelection::Unique(macro_) => return Ok(Some(macro_)),
            MacroDefinitionSelection::Ambiguous => return Ok(None),
        }
    }

    resolve_macro_use_extern_crate_fallback(env, states, state, name)
}

/// Searches build-only textual `macro_rules!` scopes for the definition visible at this call.
fn resolve_textual_macro_rules<'a>(
    env: &'a impl PathResolutionEnv,
    states: &'a FinalizeTargetStates,
    state: &TargetState,
    call: &MacroCallSite,
    name: &Name,
) -> Result<Option<ResolvedMacroDefinition<'a>>> {
    let mut module = call.module;
    let mut boundary = &call.order;

    loop {
        // In the current module, use the latest declaration that appears before the call. When we
        // climb to a parent, `boundary` becomes the child module declaration instead.
        if let Some(local_def) = state
            .textual_macro_scopes
            .latest_before(module, name, boundary)
        {
            return macro_record_for_def(
                env,
                states,
                DefId::Local(LocalDefRef {
                    target: state.target,
                    local_def,
                }),
                ScopeBindingOrigin::Direct,
            );
        }

        // A parent module contributes only declarations that appeared before the child module was
        // declared, matching the textual file view used by `macro_rules!`.
        let Some(parent) = env.parent_module(state.target, module)? else {
            return Ok(None);
        };
        let Some(parent_boundary) = state.textual_macro_scopes.module_declaration_order(module)
        else {
            return Ok(None);
        };

        boundary = parent_boundary;
        module = parent.module;
    }
}

/// Consults legacy `#[macro_use] extern crate` imports after ordinary unqualified lookup fails.
fn resolve_macro_use_extern_crate_fallback<'a>(
    env: &'a impl PathResolutionEnv,
    states: &'a FinalizeTargetStates,
    state: &TargetState,
    name: &Name,
) -> Result<Option<ResolvedMacroDefinition<'a>>> {
    let mut macros = Vec::new();

    for macro_use in &state.macro_use_imports {
        if !macro_use.selector.allows(name) {
            continue;
        }

        let import_owner = ModuleRef {
            target: state.target,
            module: macro_use.module,
        };
        for binding in visible_module_macro_bindings_with_env(
            env,
            import_owner,
            macro_use.source_module,
            name,
        )? {
            // Treat the fallback as an import-like binding. The source binding may be direct inside
            // the exporting crate, but macro-use lookup is not source-order sensitive in the caller.
            if let Some(resolved) =
                macro_record_for_def(env, states, binding.def, ScopeBindingOrigin::Import)?
            {
                macros.push(resolved);
            }
        }
    }

    Ok(unique_macro_definition(macros).into_option())
}

/// Converts a resolved macro binding into the payload needed by expansion.
fn macro_record_for_binding<'a>(
    env: &'a impl PathResolutionEnv,
    states: &'a FinalizeTargetStates,
    binding: &ScopeBinding,
) -> Result<Option<ResolvedMacroDefinition<'a>>> {
    macro_record_for_def(env, states, binding.def, binding.origin)
}

/// Converts a resolved definition id into the macro payload needed by expansion.
fn macro_record_for_def<'a>(
    env: &'a impl PathResolutionEnv,
    states: &'a FinalizeTargetStates,
    def: DefId,
    origin: ScopeBindingOrigin,
) -> Result<Option<ResolvedMacroDefinition<'a>>> {
    let DefId::Local(def_ref) = def else {
        return Ok(None);
    };
    if env.local_def_kind(def_ref)? != Some(LocalDefKind::MacroDefinition) {
        return Ok(None);
    }

    let Some(local_def) = env.local_def_data(def_ref)? else {
        return Ok(None);
    };
    let Some(data) = env.macro_definition_data(def_ref)? else {
        return Ok(None);
    };
    let order = states
        .target(def_ref.target)
        .and_then(|state| state.macro_definitions.get(&def_ref.local_def))
        .map(|record| &record.order);

    Ok(Some(ResolvedMacroDefinition {
        def_ref,
        local_def,
        data,
        order,
        origin,
    }))
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
) -> MacroDefinitionSelection<'a> {
    let mut unique: Option<ResolvedMacroDefinition<'a>> = None;
    for macro_ in macros {
        // A root `#[macro_export]` macro can appear as both its ordinary definition and exported
        // root binding. That is still one resolved macro, not an ambiguity.
        match &unique {
            Some(existing) if existing.def_ref == macro_.def_ref => {}
            Some(_) => return MacroDefinitionSelection::Ambiguous,
            None => unique = Some(macro_),
        }
    }

    match unique {
        Some(macro_) => MacroDefinitionSelection::Unique(macro_),
        None => MacroDefinitionSelection::Empty,
    }
}

enum MacroDefinitionSelection<'a> {
    Empty,
    Unique(ResolvedMacroDefinition<'a>),
    Ambiguous,
}

impl<'a> MacroDefinitionSelection<'a> {
    fn into_option(self) -> Option<ResolvedMacroDefinition<'a>> {
        match self {
            Self::Unique(macro_) => Some(macro_),
            Self::Empty | Self::Ambiguous => None,
        }
    }
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

    !(macro_.def_ref.target == target
        && macro_.local_def.module == call.module
        && macro_.order.is_some_and(|order| order > &call.order))
}

/// Returns the macro name for unqualified calls such as `foo!()`.
fn relative_single_name(path: &ImportPath) -> Option<&Name> {
    if path.absolute || path.segments.len() != 1 {
        return None;
    }

    match path.segments.first()? {
        PathSegment::Name(name) => Some(name),
        PathSegment::SelfKw
        | PathSegment::SuperKw
        | PathSegment::CrateKw
        | PathSegment::DollarCrate(_) => None,
    }
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

/// Classifies builtin macros that are known even when no user macro binding resolves.
///
/// We intentionally take a small shortcut here. Unqualified builtins and `std`/`core`-qualified
/// builtin-shaped paths cover the realistic call sites, while a fully resolution-aware builtin
/// prefix model would add a lot of complexity for rare local `std`/`core` shadowing cases.
pub(super) fn builtin_macro_disposition(path: &ImportPath) -> Option<BuiltinMacroDisposition> {
    let name = relative_single_name(path).or_else(|| {
        // Check if we have two segments and the first one is either `std` or `core`.
        let [PathSegment::Name(root), PathSegment::Name(name)] = path.segments.as_slice() else {
            return None;
        };
        matches!(root.as_str(), "std" | "core").then_some(name)
    })?;

    match name.as_str() {
        // Expression, diagnostic, or assembly builtins do not contribute named items to def-map.
        // Body lowering can later synthesize values/types for the expression-like subset.
        "asm" | "cfg" | "column" | "compile_error" | "concat" | "env" | "file" | "format_args"
        | "global_asm" | "include_bytes" | "include_str" | "line" | "llvm_asm" | "module_path"
        | "option_env" | "stringify" => Some(BuiltinMacroDisposition::IgnoredByDefMap),

        "cfg_select" => Some(BuiltinMacroDisposition::CfgSelect),
        "include" => Some(BuiltinMacroDisposition::Include),

        // `concat_idents!` has token-shaping behavior that is better handled by a dedicated
        // builtin implementation.
        "concat_idents" => Some(BuiltinMacroDisposition::Unsupported),
        _ => None,
    }
}

/// Parses the textual callee path stored in item-tree macro-call data.
pub(super) fn macro_path_from_text(
    path: &str,
    dollar_crate_target: Option<TargetRef>,
) -> Option<ImportPath> {
    let path = path.trim();
    let absolute = path.starts_with("::");
    let path = path.trim_start_matches("::");
    let mut segments = Vec::new();

    for segment in path.split("::") {
        let segment = segment.trim();
        if segment.is_empty() {
            return None;
        }
        segments.push(match segment {
            "$crate" => PathSegment::DollarCrate(dollar_crate_target?),
            "self" => PathSegment::SelfKw,
            "super" => PathSegment::SuperKw,
            "crate" => PathSegment::CrateKw,
            name => PathSegment::Name(Name::new(name)),
        });
    }

    (!segments.is_empty()).then_some(ImportPath { absolute, segments })
}
