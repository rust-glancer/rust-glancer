//! Applies and records imports during scope finalization.
//!
//! The shared scope resolver decides what named, self, and glob imports mean. This module only
//! drives that operation for crate DefMaps: it writes returned binding facts into the next
//! fixed-point snapshot, then uses the same result shape to record imports that remain unresolved
//! after the scopes stop changing.

use crate::{CrateResolutionEnv, ModuleScopeBuilder, ScopeResolver};
use rg_ir_model::{CrateRef, DefMapRef, ImportId, ImportRef, ModuleRef};

use super::{collect::CrateState, finalize::FinalizeCrateStates};

/// Unresolved import ids for one module.
type ModuleUnresolvedImports = Vec<ImportId>;

/// Unresolved imports for every module inside one crate.
type CrateUnresolvedImports = Vec<ModuleUnresolvedImports>;

/// Unresolved imports for every crate and module inside one package.
type PackageUnresolvedImports = Vec<CrateUnresolvedImports>;

/// Unresolved imports recorded after the fixed-point loop stabilizes.
///
/// Only dirty package slots contain module reports. Clean packages keep their existing frozen
/// unresolved-import state from the baseline.
pub(super) struct UnresolvedImports {
    packages: Vec<Option<PackageUnresolvedImports>>,
}

impl UnresolvedImports {
    pub(super) fn collect(
        states: &FinalizeCrateStates,
        env: &impl CrateResolutionEnv<Error = rg_package_store::PackageStoreError>,
    ) -> anyhow::Result<Self> {
        let packages = states
            .iter_packages()
            .map(|package_states| {
                package_states
                    .map(|package_states| {
                        package_states
                            .iter()
                            .map(|state| unresolved_imports_for_crate(state, env))
                            .collect::<anyhow::Result<Vec<_>>>()
                    })
                    .transpose()
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(Self { packages })
    }

    pub(super) fn crate_imports(&self, crate_ref: CrateRef) -> Option<&[Vec<ImportId>]> {
        self.packages
            .get(crate_ref.package.0)?
            .as_ref()?
            .get(crate_ref.crate_id.0)
            .map(Vec::as_slice)
    }
}

/// Apply one crate's resolved import facts to the next scope snapshot.
///
/// One directive may return several names or namespace slots. The resolver owns those decisions;
/// this function only inserts the facts into the crate's mutable scope.
pub(super) fn apply_imports(
    state: &CrateState,
    env: &impl CrateResolutionEnv<Error = rg_package_store::PackageStoreError>,
    next_scopes: &mut [ModuleScopeBuilder],
) -> anyhow::Result<()> {
    let resolver = ScopeResolver::new(env);
    for (import_id, import) in state.def_map_builder.partial().imports_with_ids() {
        let import_owner = ModuleRef::krate(state.crate_ref, import.module);
        let import_ref = ImportRef {
            origin: DefMapRef::Crate(state.crate_ref),
            import: import_id,
        };
        let resolution = resolver.resolve_import(import_owner, import_ref, import)?;
        let target_scope = next_scopes
            .get_mut(import.module.0)
            .expect("crate scope should exist for every import");
        for introduced in resolution.introduced {
            target_scope.insert_binding(&introduced.name, introduced.namespace, introduced.binding);
        }
        for binding in resolution.unnamed_traits {
            target_scope.insert_unnamed_trait_binding(binding);
        }
    }

    Ok(())
}

fn unresolved_imports_for_crate(
    state: &CrateState,
    env: &impl CrateResolutionEnv<Error = rg_package_store::PackageStoreError>,
) -> anyhow::Result<Vec<Vec<ImportId>>> {
    let mut module_imports = vec![Vec::new(); state.def_map_builder.partial().module_count()];
    let resolver = ScopeResolver::new(env);

    for (import_id, import) in state.def_map_builder.partial().imports_with_ids() {
        let import_owner = ModuleRef::krate(state.crate_ref, import.module);
        let import_ref = ImportRef {
            origin: DefMapRef::Crate(state.crate_ref),
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
