//! Applies and records imports during scope finalization.
//!
//! The shared scope resolver decides what named, self, and glob imports mean. This module only
//! drives that operation for target DefMaps: it writes returned binding facts into the next
//! fixed-point snapshot, then uses the same result shape to record imports that remain unresolved
//! after the scopes stop changing.

use rg_ir_model::{DefMapRef, ImportId, ImportRef, ModuleRef, TargetRef};
use rg_ir_storage::{ScopeResolver, TargetResolutionEnv};

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

/// Apply one target's resolved import facts to the next scope snapshot.
///
/// One directive may return several names or namespace slots. The resolver owns those decisions;
/// this function only inserts the facts into the target's mutable scope.
pub(super) fn apply_imports(
    state: &TargetState,
    env: &impl TargetResolutionEnv<Error = rg_package_store::PackageStoreError>,
    next_scopes: &mut ScopeMatrix,
) -> anyhow::Result<()> {
    let resolver = ScopeResolver::new(env);
    for (import_id, import) in state.def_map_builder.partial().imports_with_ids() {
        let import_owner = ModuleRef::target(state.target, import.module);
        let import_ref = ImportRef {
            origin: DefMapRef::Target(state.target),
            import: import_id,
        };
        let resolution = resolver.resolve_import(import_owner, import_ref, import)?;
        let target_scope = next_scopes
            .module_scope_mut(state.target, import.module)
            .expect("target scope should exist for every import");
        for introduced in resolution.introduced {
            target_scope.insert_binding(&introduced.name, introduced.namespace, introduced.binding);
        }
    }

    Ok(())
}

fn unresolved_imports_for_target(
    state: &TargetState,
    env: &impl TargetResolutionEnv<Error = rg_package_store::PackageStoreError>,
) -> anyhow::Result<Vec<Vec<ImportId>>> {
    let mut module_imports = vec![Vec::new(); state.def_map_builder.partial().module_count()];
    let resolver = ScopeResolver::new(env);

    for (import_id, import) in state.def_map_builder.partial().imports_with_ids() {
        let import_owner = ModuleRef::target(state.target, import.module);
        let import_ref = ImportRef {
            origin: DefMapRef::Target(state.target),
            import: import_id,
        };
        if !resolver
            .resolve_import(import_owner, import_ref, import)?
            .is_resolved()
        {
            module_imports
                .get_mut(import.module.0)
                .expect("import module should exist while collecting unresolved imports")
                .push(import_id);
        }
    }

    Ok(module_imports)
}
