//! Finalizes target scopes into frozen def maps.
//!
//! Collection records direct declarations and raw imports, but it intentionally leaves cross-target
//! facts unresolved. This module turns those mutable target states into immutable def maps by
//! selecting preludes, repeatedly applying imports until scopes stop changing, and then freezing
//! the settled scopes back into the target payloads. During package rebuilds, dirty package reads
//! come from fresh target states while clean package reads fall through to the old frozen database.

use anyhow::Context as _;

use rg_parse::Package;
use rg_text::{Name, PackageNameInterners};
use rg_workspace::WorkspaceMetadata;

use crate::{
    DefMap as FrozenDefMap, DefMapReadTxn, LocalDefKind, LocalDefRef, ModuleData, ModuleId,
    ModuleRef, Package as DefMapPackage, PackageSlot, TargetRef,
    collect::TargetState,
    path_resolution::{PathResolutionEnv, resolve_path_to_modules_with_env},
    scope::{ModuleScopeBuilder, ScopeEntryRef},
};

use super::imports::{UnresolvedImports, apply_imports};

/// Mutable target states for every target inside one package.
pub(super) type PackageTargetStates = Vec<TargetState>;

/// Mutable module scopes for one target.
type TargetScopeMatrix = Vec<ModuleScopeBuilder>;

/// Mutable module scopes for every target inside one package.
type PackageScopeMatrix = Vec<TargetScopeMatrix>;

/// Collected target states that must be finalized.
///
/// `Some` package slots are dirty and will be resolved/frozen. `None` slots are only valid when an
/// old `DefMapDb` baseline exists; resolution reads them from that frozen baseline instead.
pub(super) struct FinalizeTargetStates {
    packages: Vec<Option<PackageTargetStates>>,
}

impl FinalizeTargetStates {
    pub(super) fn all(packages: Vec<PackageTargetStates>) -> Self {
        Self {
            packages: packages.into_iter().map(Some).collect(),
        }
    }

    pub(super) fn empty(package_count: usize) -> Self {
        Self {
            packages: (0..package_count).map(|_| None).collect(),
        }
    }

    pub(super) fn replace_package(
        &mut self,
        package: PackageSlot,
        states: Vec<TargetState>,
    ) -> Option<()> {
        *self.packages.get_mut(package.0)? = Some(states);
        Some(())
    }

    pub(super) fn take_package(&mut self, package: PackageSlot) -> Option<Vec<TargetState>> {
        self.packages.get_mut(package.0)?.take()
    }

    pub(super) fn package(&self, package: PackageSlot) -> Option<&[TargetState]> {
        self.packages.get(package.0)?.as_deref()
    }

    pub(super) fn iter_packages(&self) -> impl Iterator<Item = Option<&[TargetState]>> + '_ {
        self.packages.iter().map(Option::as_deref)
    }

    fn target(&self, target: TargetRef) -> Option<&TargetState> {
        self.package(target.package)?.get(target.target.0)
    }

    fn iter_dirty(&self) -> impl Iterator<Item = &[TargetState]> {
        self.packages.iter().filter_map(Option::as_deref)
    }

    fn iter_dirty_mut_enumerated(&mut self) -> impl Iterator<Item = (usize, &mut [TargetState])> {
        self.packages
            .iter_mut()
            .enumerate()
            .filter_map(|(package_slot, states)| {
                states.as_deref_mut().map(|states| (package_slot, states))
            })
    }

    fn base_scopes(&self) -> ScopeMatrix {
        ScopeMatrix::from_target_states(self)
    }
}

/// Import-resolution scopes for dirty packages.
///
/// The axes are package slot, target slot, then module id. Clean package slots are absent and read
/// from the optional frozen baseline instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ScopeMatrix {
    packages: Vec<Option<PackageScopeMatrix>>,
}

impl ScopeMatrix {
    fn from_target_states(states: &FinalizeTargetStates) -> Self {
        let packages = states
            .packages
            .iter()
            .map(|package_states| {
                package_states.as_ref().map(|package_states| {
                    package_states
                        .iter()
                        .map(|state| state.base_scopes.clone())
                        .collect()
                })
            })
            .collect();

        Self { packages }
    }

    fn target_scopes(&self, target: TargetRef) -> Option<&[ModuleScopeBuilder]> {
        self.packages
            .get(target.package.0)?
            .as_ref()?
            .get(target.target.0)
            .map(Vec::as_slice)
    }

    fn module_scope(&self, module: ModuleRef) -> Option<&ModuleScopeBuilder> {
        self.target_scopes(module.target)?.get(module.module.0)
    }

    pub(super) fn module_scope_mut(
        &mut self,
        target: TargetRef,
        module: ModuleId,
    ) -> Option<&mut ModuleScopeBuilder> {
        self.packages
            .get_mut(target.package.0)?
            .as_mut()?
            .get_mut(target.target.0)?
            .get_mut(module.0)
    }
}

/// Resolution environment used while dirty package scopes are being fixed up.
///
/// Dirty package reads come from fresh target state and the current fixed-point scope snapshot.
/// Clean package reads fall through to the frozen baseline when one exists.
struct FinalizeResolutionEnv<'a> {
    old: Option<&'a DefMapReadTxn<'a>>,
    states: &'a FinalizeTargetStates,
    current_scopes: &'a ScopeMatrix,
}

impl<'a> FinalizeResolutionEnv<'a> {
    fn new(
        old: Option<&'a DefMapReadTxn<'a>>,
        states: &'a FinalizeTargetStates,
        current_scopes: &'a ScopeMatrix,
    ) -> Self {
        Self {
            old,
            states,
            current_scopes,
        }
    }
}

impl PathResolutionEnv for FinalizeResolutionEnv<'_> {
    fn extern_root(
        &self,
        target: TargetRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, rg_package_store::PackageStoreError> {
        if let Some(state) = self.states.target(target) {
            return Ok(state.implicit_roots.get(name).copied());
        }

        Ok(self
            .old
            .map(|old| old.def_map(target))
            .transpose()?
            .flatten()
            .and_then(|def_map| def_map.extern_prelude().get(name).copied()))
    }

    fn prelude_module(
        &self,
        target: TargetRef,
    ) -> Result<Option<ModuleRef>, rg_package_store::PackageStoreError> {
        if let Some(state) = self.states.target(target) {
            return Ok(state.prelude);
        }

        Ok(self
            .old
            .map(|old| old.def_map(target))
            .transpose()?
            .flatten()
            .and_then(|def_map| def_map.prelude()))
    }

    fn root_module(
        &self,
        target: TargetRef,
    ) -> Result<Option<ModuleRef>, rg_package_store::PackageStoreError> {
        let module = if let Some(state) = self.states.target(target) {
            state.def_map.root_module()
        } else {
            self.old
                .map(|old| old.def_map(target))
                .transpose()?
                .flatten()
                .and_then(|def_map| def_map.root_module())
        };

        Ok(module.map(|module| ModuleRef { target, module }))
    }

    fn module_data(
        &self,
        module_ref: ModuleRef,
    ) -> Result<Option<&ModuleData>, rg_package_store::PackageStoreError> {
        if let Some(state) = self.states.target(module_ref.target) {
            return Ok(state.def_map.module(module_ref.module));
        }

        Ok(self
            .old
            .map(|old| old.def_map(module_ref.target))
            .transpose()?
            .flatten()
            .and_then(|def_map| def_map.module(module_ref.module)))
    }

    fn module_scope_entry<'a>(
        &'a self,
        module_ref: ModuleRef,
        name: &str,
    ) -> Result<Option<ScopeEntryRef<'a>>, rg_package_store::PackageStoreError> {
        if self.states.package(module_ref.target.package).is_some() {
            return Ok(self
                .current_scopes
                .module_scope(module_ref)
                .and_then(|scope| scope.entry(name)));
        }

        Ok(self
            .module_data(module_ref)?
            .and_then(|module| module.scope.entry(name))
            .map(|entry| entry.as_ref()))
    }

    fn module_scope_entries<'a>(
        &'a self,
        module_ref: ModuleRef,
    ) -> Result<Vec<(&'a Name, ScopeEntryRef<'a>)>, rg_package_store::PackageStoreError> {
        if self.states.package(module_ref.target.package).is_some() {
            return Ok(self
                .current_scopes
                .module_scope(module_ref)
                .map(|scope| scope.entries().collect())
                .unwrap_or_default());
        }

        Ok(self
            .module_data(module_ref)?
            .map(|module| {
                module
                    .scope
                    .entries()
                    .map(|(name, entry)| (name, entry.as_ref()))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn local_def_kind(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<LocalDefKind>, rg_package_store::PackageStoreError> {
        if let Some(state) = self.states.target(local_def_ref.target) {
            return Ok(state
                .def_map
                .local_defs
                .get(local_def_ref.local_def)
                .map(|local_def| local_def.kind));
        }

        Ok(self
            .old
            .map(|old| old.local_def(local_def_ref))
            .transpose()?
            .flatten()
            .map(|local_def| local_def.kind))
    }
}

pub(super) fn finalize_target_states(
    old: Option<&DefMapReadTxn<'_>>,
    workspace: &WorkspaceMetadata,
    packages: &[Package],
    target_states: &mut FinalizeTargetStates,
    interners: &mut PackageNameInterners,
) -> anyhow::Result<()> {
    select_preludes(old, workspace, packages, target_states, interners)
        .context("while attempting to select target preludes")?;
    finalize_scopes(old, target_states).context("while attempting to resolve target scopes")
}

pub(super) fn freeze_package_states(
    package: &Package,
    package_states: &[TargetState],
) -> DefMapPackage {
    DefMapPackage {
        name: package.package_name().to_string(),
        target_names: rg_arena::Arena::from_vec(
            package_states
                .iter()
                .map(|state| state.target_name.clone())
                .collect(),
        ),
        targets: rg_arena::Arena::from_vec(
            package_states
                .iter()
                .map(freeze_target_state)
                .collect::<Vec<_>>(),
        ),
    }
}

fn select_preludes(
    old: Option<&DefMapReadTxn<'_>>,
    workspace: &WorkspaceMetadata,
    packages: &[Package],
    states: &mut FinalizeTargetStates,
    interners: &mut PackageNameInterners,
) -> anyhow::Result<()> {
    let base_scopes = states.base_scopes();
    let env = FinalizeResolutionEnv::new(old, states, &base_scopes);
    let mut selected_preludes = packages
        .iter()
        .enumerate()
        .map(|(package_slot, _)| {
            states
                .package(PackageSlot(package_slot))
                .map(|states| vec![None; states.len()])
        })
        .collect::<Vec<_>>();

    for (package_slot, package) in packages.iter().enumerate() {
        let Some(package_states) = states.package(PackageSlot(package_slot)) else {
            continue;
        };
        let workspace_package = workspace.package(package.id()).with_context(|| {
            format!(
                "while attempting to fetch workspace metadata for package {}",
                package.id()
            )
        })?;
        let interner = interners.package_mut(package_slot).with_context(|| {
            format!("while attempting to fetch name interner for package {package_slot}")
        })?;
        let prelude_path = crate::ImportPath::standard_prelude(workspace_package.edition, interner);

        for (target_slot, state) in package_states.iter().enumerate() {
            let Some(root_module) = state.def_map.root_module() else {
                continue;
            };
            let Some(prelude_module) =
                resolve_path_to_modules_with_env(&env, state.target, root_module, &prelude_path)?
                    .into_iter()
                    .next()
            else {
                continue;
            };

            let package_preludes = selected_preludes[package_slot]
                .as_mut()
                .expect("prelude slots should exist for every dirty package");
            package_preludes[target_slot] = Some(prelude_module);
        }
    }

    for (package_slot, package_states) in states.iter_dirty_mut_enumerated() {
        let package_preludes = selected_preludes[package_slot]
            .as_ref()
            .expect("prelude slots should exist for every dirty package");
        for (target_slot, state) in package_states.iter_mut().enumerate() {
            state.prelude = package_preludes[target_slot];
        }
    }

    Ok(())
}

/// Resolves imports until every dirty target scope stops changing.
///
/// Imports can depend on names introduced by other imports, so one pass is not enough. Each pass
/// reads from the previous scope matrix and writes into a fresh matrix seeded from base scopes.
fn finalize_scopes(
    old: Option<&DefMapReadTxn<'_>>,
    states: &mut FinalizeTargetStates,
) -> anyhow::Result<()> {
    let mut current_scopes = states.base_scopes();

    loop {
        let mut next_scopes = states.base_scopes();

        // Every iteration starts from the directly declared names, then layers import-derived
        // bindings on top of that snapshot.
        let env = FinalizeResolutionEnv::new(old, states, &current_scopes);
        for package_states in states.iter_dirty() {
            for state in package_states {
                apply_imports(state, &env, &mut next_scopes).with_context(|| {
                    format!(
                        "while attempting to resolve imports for {}",
                        state.target_name
                    )
                })?;
            }
        }

        if next_scopes == current_scopes {
            freeze_resolved_scopes(old, states, &current_scopes)?;
            return Ok(());
        }

        current_scopes = next_scopes;
    }
}

fn freeze_resolved_scopes(
    old: Option<&DefMapReadTxn<'_>>,
    states: &mut FinalizeTargetStates,
    current_scopes: &ScopeMatrix,
) -> anyhow::Result<()> {
    // Once the import graph reaches a fixed point, freeze the resolved scopes into the public
    // def-map payload and preserve unresolved imports for query consumers.
    let unresolved_imports = {
        let env = FinalizeResolutionEnv::new(old, states, current_scopes);
        UnresolvedImports::collect(states, &env)?
    };

    for (_, package_states) in states.iter_dirty_mut_enumerated() {
        for state in package_states {
            freeze_target_scopes(state, current_scopes, &unresolved_imports);
        }
    }

    Ok(())
}

fn freeze_target_scopes(
    state: &mut TargetState,
    current_scopes: &ScopeMatrix,
    unresolved_imports: &UnresolvedImports,
) {
    let final_scopes = current_scopes
        .target_scopes(state.target)
        .expect("final scopes should exist for every dirty target");
    let final_unresolved_imports = unresolved_imports
        .target_imports(state.target)
        .expect("unresolved imports should exist for every dirty target");

    for (module_idx, scope) in final_scopes.iter().enumerate() {
        let module = state
            .def_map
            .modules
            .get_mut(ModuleId(module_idx))
            .expect("module should exist for every final dirty scope");
        module.scope = scope.freeze();
        module.unresolved_imports = final_unresolved_imports
            .get(module_idx)
            .expect("unresolved imports should exist for every dirty module")
            .clone();
    }
}

fn freeze_target_state(state: &TargetState) -> FrozenDefMap {
    let mut def_map = state.def_map.clone();

    // The same implicit roots used by import resolution are still needed by later frozen path
    // queries. Keep them as an extern prelude rather than pretending they are child modules of the
    // crate root.
    def_map.set_extern_prelude(state.implicit_roots.clone());
    def_map.set_prelude(state.prelude);
    def_map
}
