//! Applies and records imports during scope finalization.
//!
//! Import resolution is a fixed-point process driven by `scope.rs`. This module owns the work done
//! inside one pass: resolving named/self/glob imports against the previous scope snapshot, writing
//! the imported bindings into the next scope snapshot, and recording imports that still fail once
//! the fixed point has stabilized.

use rg_ir_model::{ImportId, ModuleRef, TargetRef};
use rg_ir_storage::{
    GlobImportSource, ImportKind, Namespace, PathResolver, ScopeBinding, ScopeBindingOrigin,
    TargetResolutionEnv,
};

use super::{
    collect::TargetState,
    finalize::{FinalizeTargetStates, ScopeMatrix},
};

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
        env: &impl TargetResolutionEnv<Error = rg_package_store::PackageStoreError>,
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

    pub(super) fn target_imports(&self, target: TargetRef) -> Option<&[Vec<ImportId>]> {
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
    env: &impl TargetResolutionEnv<Error = rg_package_store::PackageStoreError>,
    next_scopes: &mut ScopeMatrix,
) -> anyhow::Result<()> {
    let resolver = PathResolver::new(env);
    for import in state.def_map_builder.partial().imports().iter() {
        let import_owner = ModuleRef::target(state.target, import.module);
        match import.kind {
            ImportKind::Glob => {
                let glob_sources = resolver.import_glob_sources(import_owner, &import.path)?;

                for glob_source in glob_sources {
                    let target_scope = next_scopes
                        .module_scope_mut(state.target, import.module)
                        .expect("target scope should exist for every import");

                    match glob_source {
                        GlobImportSource::Module(source_module) => {
                            let source_scope =
                                resolver.visible_scope(import_owner, source_module)?;

                            // Visibility is attached to the binding introduced by the glob import,
                            // not to the original definition.
                            for (name, entry) in source_scope.entries() {
                                target_scope.copy_visible_bindings(
                                    name,
                                    entry,
                                    import.visibility.clone(),
                                    import_owner,
                                );
                            }
                        }
                        GlobImportSource::Enum(enum_def) => {
                            for (name, binding) in
                                resolver.visible_enum_variant_bindings(import_owner, enum_def)?
                            {
                                target_scope.insert_binding(
                                    &name,
                                    Namespace::Values,
                                    ScopeBinding {
                                        def: binding.def,
                                        visibility: import.visibility.clone(),
                                        owner: import_owner,
                                        origin: ScopeBindingOrigin::Import,
                                    },
                                );
                            }
                        }
                    }
                }
            }
            ImportKind::Named | ImportKind::SelfImport => {
                let resolved_defs = resolver.import_defs(import_owner, &import.path)?;

                let Some(binding_name) = import.binding_name() else {
                    continue;
                };
                let target_scope = next_scopes
                    .module_scope_mut(state.target, import.module)
                    .expect("target scope should exist for every import");

                for resolved_def in resolved_defs {
                    // Resolution is namespace-aware, but the target textual name is shared across
                    // namespaces inside one scope entry.
                    let Some(namespace) = resolver.namespace_for_def(resolved_def)? else {
                        continue;
                    };
                    target_scope.insert_binding(
                        &binding_name,
                        namespace,
                            ScopeBinding {
                                def: resolved_def,
                                visibility: import.visibility.clone(),
                                owner: import_owner,
                                origin: ScopeBindingOrigin::Import,
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
    env: &impl TargetResolutionEnv<Error = rg_package_store::PackageStoreError>,
) -> anyhow::Result<Vec<Vec<ImportId>>> {
    let mut module_imports = vec![Vec::new(); state.def_map_builder.partial().module_count()];
    let resolver = PathResolver::new(env);

    for (import_id, import) in state.def_map_builder.partial().imports_with_ids() {
        let import_owner = ModuleRef::target(state.target, import.module);
        let is_unresolved = match import.kind {
            ImportKind::Glob => resolver
                .import_glob_sources(import_owner, &import.path)?
                .is_empty(),
            ImportKind::Named | ImportKind::SelfImport => resolver
                .import_defs(import_owner, &import.path)?
                .is_empty(),
        };
        if is_unresolved {
            module_imports
                .get_mut(import.module.0)
                .expect("import module should exist while collecting unresolved imports")
                .push(import_id);
        }
    }

    Ok(module_imports)
}
