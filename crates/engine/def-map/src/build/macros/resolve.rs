//! Resolves macro-call paths against the current def-map scope snapshot.
//!
//! Macro resolution mostly reuses normal path resolution, but single-segment calls need direct
//! macro-namespace lookup so locally imported macros can shadow builtin names.

use anyhow::Result;

use rg_text::Name;

use crate::{
    DefId, ImportPath, LocalDefData, LocalDefKind, LocalDefRef, MacroDefinitionData, ModuleRef,
    PathSegment,
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
        // Unqualified macro calls first look in the macro namespace of the current module. This
        // also keeps user-defined `include!`-style names from being mistaken for builtins.
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

    Ok(unique_macro_definition(macros))
}

fn resolve_single_name_macro<'a>(
    env: &'a impl PathResolutionEnv,
    states: &'a FinalizeTargetStates,
    state: &TargetState,
    call: &MacroCallSite,
    name: &Name,
) -> Result<Option<ResolvedMacroDefinition<'a>>> {
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

    let mut macros = Vec::new();
    for binding in entry.macros() {
        if let Some(resolved) = macro_record_for_def(env, states, binding.def)? {
            macros.push(resolved);
        }
    }

    Ok(unique_macro_definition(macros))
}

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

fn unique_macro_definition(
    macros: Vec<ResolvedMacroDefinition<'_>>,
) -> Option<ResolvedMacroDefinition<'_>> {
    match macros.as_slice() {
        [_] => macros.into_iter().next(),
        [] => None,
        _ => None,
    }
}

fn relative_single_name(path: &ImportPath) -> Option<&Name> {
    if path.absolute || path.segments.len() != 1 {
        return None;
    }

    match path.segments.first()? {
        PathSegment::Name(name) => Some(name),
        PathSegment::SelfKw | PathSegment::SuperKw | PathSegment::CrateKw => None,
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
pub(super) fn macro_path_from_text(path: &str) -> Option<ImportPath> {
    let absolute = path.starts_with("::");
    let path = path.trim_start_matches("::");
    let mut segments = Vec::new();

    for segment in path.split("::") {
        if segment.is_empty() {
            return None;
        }
        segments.push(match segment {
            "self" => PathSegment::SelfKw,
            "super" => PathSegment::SuperKw,
            "crate" => PathSegment::CrateKw,
            name => PathSegment::Name(Name::new(name)),
        });
    }

    (!segments.is_empty()).then_some(ImportPath { absolute, segments })
}
