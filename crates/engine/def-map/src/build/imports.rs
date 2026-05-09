//! Applies and records imports during scope finalization.
//!
//! Import resolution is a fixed-point process driven by `scope.rs`. This module owns the work done
//! inside one pass: resolving named/self/glob imports against the previous scope snapshot, writing
//! the imported bindings into the next scope snapshot, and recording imports that still fail once
//! the fixed point has stabilized.

use crate::{
    ImportData, ImportId, ImportKind, ModuleRef, ScopeBinding,
    collect::TargetState,
    path_resolution::{
        PathResolutionEnv, namespace_for_def_with_env, resolve_path_to_defs_with_env,
        resolve_path_to_modules_with_env, visible_module_scope_entry_set_with_env,
    },
};

use super::finalize::{FinalizeTargetStates, ScopeMatrix};

/// Unresolved import ids for one module.
type ModuleUnresolvedImports = Vec<ImportId>;

/// Unresolved imports for every module inside one target.
type TargetUnresolvedImports = Vec<ModuleUnresolvedImports>;

/// Unresolved imports for every target and module inside one package.
type PackageUnresolvedImports = Vec<TargetUnresolvedImports>;

/// Unresolved imports recorded after the fixed-point loop stabilizes.
///
/// Only dirty package slots contain module reports. Clean packages keep their existing frozen
/// unresolved-import state from the baseline.
pub(super) struct UnresolvedImports {
    packages: Vec<Option<PackageUnresolvedImports>>,
}

impl UnresolvedImports {
    pub(super) fn collect(
        states: &FinalizeTargetStates,
        env: &impl PathResolutionEnv,
    ) -> anyhow::Result<Self> {
        let packages = states
            .iter_packages()
            .map(|package_states| {
                package_states
                    .map(|package_states| {
                        package_states
                            .iter()
                            .map(|state| unresolved_imports_for_target(state, env))
                            .collect::<anyhow::Result<Vec<_>>>()
                    })
                    .transpose()
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(Self { packages })
    }

    pub(super) fn target_imports(&self, target: crate::TargetRef) -> Option<&[Vec<ImportId>]> {
        self.packages
            .get(target.package.0)?
            .as_ref()?
            .get(target.target.0)
            .map(Vec::as_slice)
    }
}

/// Applies one target's imports using the previously computed scope snapshot.
///
/// Named/self imports add a binding under one textual name. Glob imports copy every visible
/// binding from the source module into the target module.
pub(super) fn apply_imports(
    state: &TargetState,
    env: &impl PathResolutionEnv,
    next_scopes: &mut ScopeMatrix,
) -> anyhow::Result<()> {
    for import in state.def_map.imports().iter() {
        match import.kind {
            ImportKind::Glob => {
                let source_modules = resolve_path_to_modules_with_env(
                    env,
                    state.target,
                    import.module,
                    &import.path,
                )?;

                for source_module in source_modules {
                    let import_owner = ModuleRef {
                        target: state.target,
                        module: import.module,
                    };
                    let source_scope =
                        visible_module_scope_entry_set_with_env(env, import_owner, source_module)?;
                    let target_scope = next_scopes
                        .module_scope_mut(state.target, import.module)
                        .expect("target scope should exist for every import");

                    // Visibility is attached to the binding introduced by the glob import, not to
                    // the original definition.
                    for (name, entry) in source_scope.entries() {
                        target_scope.copy_visible_bindings(
                            name,
                            entry,
                            import.visibility.clone(),
                            import_owner,
                        );
                    }
                }
            }
            ImportKind::Named | ImportKind::SelfImport => {
                let resolved_defs =
                    resolve_path_to_defs_with_env(env, state.target, import.module, &import.path)?;

                let Some(binding_name) = import.binding_name() else {
                    continue;
                };
                let target_scope = next_scopes
                    .module_scope_mut(state.target, import.module)
                    .expect("target scope should exist for every import");

                for resolved_def in resolved_defs {
                    // Resolution is namespace-aware, but the target textual name is shared across
                    // namespaces inside one scope entry.
                    let Some(namespace) = namespace_for_def_with_env(env, resolved_def)? else {
                        continue;
                    };
                    target_scope.insert_binding(
                        &binding_name,
                        namespace,
                        ScopeBinding {
                            def: resolved_def,
                            visibility: import.visibility.clone(),
                            owner: ModuleRef {
                                target: state.target,
                                module: import.module,
                            },
                        },
                    );
                }
            }
        }
    }

    Ok(())
}

fn unresolved_imports_for_target(
    state: &TargetState,
    env: &impl PathResolutionEnv,
) -> anyhow::Result<Vec<Vec<ImportId>>> {
    let mut module_imports = vec![Vec::new(); state.def_map.modules.len()];

    for (import_id, import) in state.def_map.imports.iter_with_ids() {
        if import_is_unresolved(state, env, import)? {
            module_imports
                .get_mut(import.module.0)
                .expect("import module should exist while collecting unresolved imports")
                .push(import_id);
        }
    }

    Ok(module_imports)
}

/// Checks whether one import failed to resolve, independent of whether it introduces a binding.
fn import_is_unresolved(
    state: &TargetState,
    env: &impl PathResolutionEnv,
    import: &ImportData,
) -> anyhow::Result<bool> {
    match import.kind {
        ImportKind::Glob => {
            Ok(
                resolve_path_to_modules_with_env(env, state.target, import.module, &import.path)?
                    .is_empty(),
            )
        }
        ImportKind::Named | ImportKind::SelfImport => {
            Ok(
                resolve_path_to_defs_with_env(env, state.target, import.module, &import.path)?
                    .is_empty(),
            )
        }
    }
}
