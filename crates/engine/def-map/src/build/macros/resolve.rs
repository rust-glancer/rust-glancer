//! Resolves macro-call paths against the current def-map scope snapshot.
//!
//! Macro resolution mostly reuses normal path resolution, but single-segment calls need direct
//! macro-namespace lookup so locally imported macros can shadow builtin names.

use anyhow::Result;

use rg_text::Name;

use crate::{
    DefId, ImportPath, LocalDefData, LocalDefKind, LocalDefRef, MacroDefinitionData, ModuleRef,
    PathSegment, TargetRef,
    build::{collect::TargetState, finalize::FinalizeTargetStates},
    query::path_resolution::{PathResolutionEnv, resolve_path_to_defs_with_env},
};

use super::{ItemOrder, MacroCallSite};

/// Macro definition resolved through the ordinary macro namespace.
pub(super) struct ResolvedMacroDefinition<'a> {
    pub(super) def_ref: LocalDefRef,
    pub(super) local_def: &'a LocalDefData,
    pub(super) data: &'a MacroDefinitionData,
    pub(super) order: Option<&'a ItemOrder>,
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

    // Qualified calls follow ordinary path resolution, then filter the result set down to macro
    // definitions that still have their macro payload available in target state.
    let resolved_defs = resolve_path_to_defs_with_env(env, state.target, call.module, path)?;
    let mut macros = Vec::new();

    for def in resolved_defs {
        if let Some(resolved) = macro_record_for_def(env, states, def)? {
            macros.push(resolved);
        }
    }

    Ok(unique_visible_macro_definition(macros, state.target, call))
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

    // From here on we use the ordinary macro namespace as an approximation for imported/exported
    // macros. That may keep resolving some malformed source states that rustc rejects, but those
    // states are already invalid Rust; precisely modeling them would complicate expansion without
    // improving valid-code behavior.
    let Some(entry) = env.module_scope_entry(
        ModuleRef {
            target: state.target,
            module: call.module,
        },
        name.as_str(),
    )?
    else {
        return Ok(None);
    };

    // If no textual macro applies, imports/reexports and exported macros are represented as normal
    // macro namespace bindings in the current resolved scope snapshot.
    let mut macros = Vec::new();
    for binding in entry.macros() {
        if let Some(resolved) = macro_record_for_def(env, states, binding.def)? {
            macros.push(resolved);
        }
    }

    Ok(unique_visible_macro_definition(macros, state.target, call))
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

/// Converts a resolved definition id into the macro payload needed by expansion.
fn macro_record_for_def<'a>(
    env: &'a impl PathResolutionEnv,
    states: &'a FinalizeTargetStates,
    def: DefId,
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
    }))
}

/// Selects a single visible macro from ordinary namespace resolution results.
fn unique_visible_macro_definition<'a>(
    macros: Vec<ResolvedMacroDefinition<'a>>,
    target: TargetRef,
    call: &MacroCallSite,
) -> Option<ResolvedMacroDefinition<'a>> {
    let macros = macros
        .into_iter()
        .filter(|macro_| macro_definition_is_visible_by_order(macro_, target, call))
        .collect::<Vec<_>>();

    match macros.as_slice() {
        [_] => macros.into_iter().next(),
        [] => None,
        _ => None,
    }
}

/// Filters ordinary namespace candidates that are textually later than the call site.
fn macro_definition_is_visible_by_order(
    macro_: &ResolvedMacroDefinition<'_>,
    target: TargetRef,
    call: &MacroCallSite,
) -> bool {
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

/// Identifies builtin macros we intentionally do not expand in this milestone.
pub(super) fn is_unsupported_builtin_macro_path(path: &ImportPath) -> bool {
    let Some(name) = relative_single_name(path) else {
        return false;
    };

    matches!(
        name.as_str(),
        "asm"
            | "cfg"
            | "column"
            | "compile_error"
            | "concat"
            | "concat_idents"
            | "env"
            | "file"
            | "format_args"
            | "global_asm"
            | "include"
            | "include_bytes"
            | "include_str"
            | "line"
            | "llvm_asm"
            | "module_path"
            | "option_env"
            | "stringify"
    )
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
