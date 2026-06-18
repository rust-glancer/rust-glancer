//! Finalizes target scopes into frozen def maps.
//!
//! Collection records direct declarations and raw imports, but it intentionally leaves cross-target
//! facts unresolved. This module turns those mutable target states into immutable def maps by
//! selecting preludes, repeatedly applying imports until scopes stop changing, and then freezing
//! the settled scopes back into the target payloads. During package rebuilds, dirty package reads
//! come from fresh target states while clean package reads fall through to the old frozen database.

mod clean;
mod rebuild;

use anyhow::Context as _;

use rg_ir_model::{DefMapRef, LocalDefRef, ModuleId, ModuleRef, TargetRef};
use rg_ir_storage::{
    DefMap, ImportPath, LocalDefData, MacroDefinitionData, ModuleData, ModuleScopeBuilder,
    PackageDefMaps as DefMapPackage, PathResolver, ScopeEntryRef, ScopeResolutionEnv, TargetData,
    TargetResolutionEnv,
};
use rg_item_tree::ItemTreeDb;
use rg_parse::Package;
use rg_text::{Name, PackageNameInterners};
use rg_workspace::WorkspaceMetadata;

use crate::{DefMapReadTxn, PackageSlot, profile::metric};

use super::{
    DefMapPerformancePreference,
    collect::TargetState,
    imports::{UnresolvedImports, apply_imports},
    macros::{
        MAX_MACRO_EXPANSION_PASSES, MacroExpansionCache, MacroExpansionCursors,
        MacroExpansionExecutor, MacroExpansionScan, apply_expansion_attempts,
        collect_expansion_attempts, expand_expansion_attempts,
        mark_retryable_macros_skipped_by_limit,
    },
};

pub(crate) use self::{clean::build_db, rebuild::rebuild_packages};

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

    pub(super) fn target(&self, target: TargetRef) -> Option<&TargetState> {
        self.package(target.package)?.get(target.target.0)
    }

    pub(super) fn target_mut(&mut self, target: TargetRef) -> Option<&mut TargetState> {
        self.packages
            .get_mut(target.package.0)?
            .as_deref_mut()?
            .get_mut(target.target.0)
    }

    pub(super) fn iter_dirty(&self) -> impl Iterator<Item = &[TargetState]> {
        self.packages.iter().filter_map(Option::as_deref)
    }

    pub(super) fn iter_dirty_mut(&mut self) -> impl Iterator<Item = &mut [TargetState]> {
        self.packages.iter_mut().filter_map(Option::as_deref_mut)
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
        self.target_scopes(module.origin.as_target_ref()?)?
            .get(module.module.0)
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

    pub(super) fn push_module_scope(
        &mut self,
        target: TargetRef,
        scope: ModuleScopeBuilder,
    ) -> Option<()> {
        self.packages
            .get_mut(target.package.0)?
            .as_mut()?
            .get_mut(target.target.0)?
            .push(scope);
        Some(())
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

impl ScopeResolutionEnv for FinalizeResolutionEnv<'_> {
    type Error = rg_package_store::PackageStoreError;

    fn module_data(
        &self,
        module_ref: ModuleRef,
    ) -> Result<Option<&ModuleData>, rg_package_store::PackageStoreError> {
        if let Some(target) = module_ref.origin.as_target_ref()
            && let Some(state) = self.states.target(target)
        {
            return Ok(state.def_map_builder.partial().module(module_ref.module));
        }

        let Some(target) = module_ref.origin.as_target_ref() else {
            return Ok(None);
        };
        Ok(self
            .old
            .map(|old| old.def_map(target))
            .transpose()?
            .flatten()
            .and_then(|def_map| def_map.module(module_ref.module)))
    }

    fn module_scope_entry<'a>(
        &'a self,
        module_ref: ModuleRef,
        name: &str,
    ) -> Result<Option<ScopeEntryRef<'a>>, rg_package_store::PackageStoreError> {
        if module_ref
            .origin
            .as_target_ref()
            .is_some_and(|target| self.states.package(target.package).is_some())
        {
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
        if module_ref
            .origin
            .as_target_ref()
            .is_some_and(|target| self.states.package(target.package).is_some())
        {
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

    fn local_def_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&LocalDefData>, rg_package_store::PackageStoreError> {
        if let Some(target) = local_def_ref.origin.as_target_ref()
            && let Some(state) = self.states.target(target)
        {
            return Ok(state
                .def_map_builder
                .partial()
                .local_def(local_def_ref.local_def));
        }

        let Some(target) = local_def_ref.origin.as_target_ref() else {
            return Ok(None);
        };
        self.old
            .map(|old| {
                Ok(old
                    .def_map(target)?
                    .and_then(|def_map| def_map.local_def(local_def_ref.local_def)))
            })
            .transpose()
            .map(Option::flatten)
    }

    fn macro_definition_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&MacroDefinitionData>, rg_package_store::PackageStoreError> {
        if let Some(target) = local_def_ref.origin.as_target_ref()
            && let Some(state) = self.states.target(target)
        {
            return Ok(state
                .def_map_builder
                .partial()
                .macro_definition(local_def_ref.local_def));
        }

        let Some(target) = local_def_ref.origin.as_target_ref() else {
            return Ok(None);
        };
        Ok(self
            .old
            .map(|old| old.def_map(target))
            .transpose()?
            .flatten()
            .and_then(|def_map| def_map.macro_definition(local_def_ref.local_def)))
    }
}

impl TargetResolutionEnv for FinalizeResolutionEnv<'_> {
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
            .map(|old| old.package(target.package))
            .transpose()?
            .and_then(|package| {
                package
                    .target_data(target.target)
                    .and_then(|data| data.extern_prelude().get(name).copied())
            }))
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
            .map(|old| old.package(target.package))
            .transpose()?
            .and_then(|package| {
                package
                    .target_data(target.target)
                    .and_then(|data| data.prelude())
            }))
    }

    fn root_module(
        &self,
        target: TargetRef,
    ) -> Result<Option<ModuleRef>, rg_package_store::PackageStoreError> {
        let module = if let Some(state) = self.states.target(target) {
            Some(state.root_module)
        } else {
            self.old
                .map(|old| old.package(target.package))
                .transpose()?
                .and_then(|package| {
                    package
                        .target_data(target.target)
                        .and_then(|data| data.root_module())
                })
        };

        Ok(module.map(|module| ModuleRef {
            origin: DefMapRef::Target(target),
            module,
        }))
    }
}

/// Completes mutable target states after collection and before freezing.
///
/// Collection records only local facts. This step attaches the edition prelude for each target,
/// resolves imports and item-position macros against the package graph, and writes the final
/// module scopes back into the collected states.
#[allow(clippy::too_many_arguments)]
pub(super) fn finalize_target_states(
    old: Option<&DefMapReadTxn<'_>>,
    workspace: &WorkspaceMetadata,
    packages: &[Package],
    item_tree: &ItemTreeDb,
    target_states: &mut FinalizeTargetStates,
    interners: &mut PackageNameInterners,
    performance_preference: DefMapPerformancePreference,
) -> anyhow::Result<()> {
    // Prelude selection needs the directly declared root modules and implicit extern roots, but it
    // must happen before import resolution because prelude imports participate in normal lookup.
    select_preludes(old, workspace, packages, target_states, interners)
        .context("while attempting to select target preludes")?;

    // Once each target knows its prelude, imports and item-position macros can be resolved through
    // the shared fixed-point loop.
    finalize_scopes(
        old,
        item_tree,
        target_states,
        interners,
        performance_preference,
    )
    .context("while attempting to resolve target scopes")
}

/// Freezes collected target states into the package payload stored by `DefMapDb`.
pub(super) fn freeze_package(package: &Package, package_states: &[TargetState]) -> DefMapPackage {
    DefMapPackage::new(
        package.package_name().to_string(),
        package_states
            .iter()
            .map(|state| state.target_name.clone())
            .collect(),
        package_states.iter().map(freeze_target_data).collect(),
        package_states.iter().map(freeze_target_state).collect(),
    )
}

/// Selects the standard prelude module visible from each dirty target.
///
/// The prelude path depends on the target edition, and the module it resolves to can live in a
/// clean package. Resolution therefore uses the same dirty-state-plus-old-baseline environment as
/// the later import fixed point.
fn select_preludes(
    old: Option<&DefMapReadTxn<'_>>,
    workspace: &WorkspaceMetadata,
    packages: &[Package],
    states: &mut FinalizeTargetStates,
    interners: &mut PackageNameInterners,
) -> anyhow::Result<()> {
    // Prelude lookup only needs directly declared names and implicit extern roots. Using base
    // scopes here keeps the operation independent from later import and macro expansion passes.
    let base_scopes = states.base_scopes();
    let env = FinalizeResolutionEnv::new(old, states, &base_scopes);

    // Store selected preludes out-of-band first so path resolution can borrow all target states
    // immutably while we inspect roots across packages.
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
        // Each target resolves its edition prelude from its own crate root. Targets without a root
        // module are malformed enough that later phases will simply see no prelude.
        for (target_slot, state) in package_states.iter().enumerate() {
            let mut prelude_module = None;

            // Normal crates use `std` when available. No-std-shaped crates still need the same
            // edition prelude, but rooted at `core`. The core crate itself has a crate-local
            // `prelude` module, so resolve that shape relatively during this early pass.
            // TODO: Parse crate-level `#![no_std]` and use it to select `core` prelude directly
            // and avoid exposing `std` as an automatic extern root for that crate.
            let prelude_paths = [
                Some(ImportPath::standard_prelude(
                    "std",
                    workspace_package.edition,
                    interner,
                )),
                Some(ImportPath::standard_prelude(
                    "core",
                    workspace_package.edition,
                    interner,
                )),
                (workspace_package.name == "core").then(|| {
                    ImportPath::crate_relative_standard_prelude(workspace_package.edition, interner)
                }),
            ];

            for prelude_path in prelude_paths.into_iter().flatten() {
                prelude_module = PathResolver::new(&env)
                    .import_modules(state.target, state.root_module, &prelude_path)?
                    .into_iter()
                    .next();
                if prelude_module.is_some() {
                    break;
                }
            }

            let Some(prelude_module) = prelude_module else {
                continue;
            };

            let package_preludes = selected_preludes[package_slot]
                .as_mut()
                .expect("prelude slots should exist for every dirty package");
            package_preludes[target_slot] = Some(prelude_module);
        }
    }

    // Apply the selected modules after lookup is done so future import resolution can consult the
    // prelude through `TargetResolutionEnv::prelude_module`.
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

/// Resolves imports and item-position macros until every dirty target scope stops changing.
///
/// Imports can depend on names introduced by other imports, and macro calls can depend on imports
/// that make the macro definition visible. This function therefore runs a small fixed-point loop:
/// resolve imports against the current target states, expand the macros that are now visible,
/// splice generated items back into the mutable target states, and refresh imports whenever those
/// generated items may have introduced new imports or exported names.
fn finalize_scopes(
    old: Option<&DefMapReadTxn<'_>>,
    item_tree: &ItemTreeDb,
    states: &mut FinalizeTargetStates,
    interners: &mut PackageNameInterners,
    performance_preference: DefMapPerformancePreference,
) -> anyhow::Result<()> {
    let mut macro_cache = MacroExpansionCache::default();
    let mut macro_expansion_executor = None;
    let mut expansion_passes = 0;
    metric::EXPANSION_PASS_LIMIT.record_count(MAX_MACRO_EXPANSION_PASSES);

    loop {
        metric::ROUNDS.inc();

        // Start each round by letting imports settle over the declarations collected so far. This
        // includes items generated by earlier macro rounds, because those items were written back
        // into `states` before the next round begins.
        let timer = metric::TIMING_RESOLVE_IMPORT_SCOPES.start_timer();
        let mut current_scopes = resolve_import_scopes(old, states)?;
        timer.finish();

        // Macro expansion can introduce more macro calls that are visible in the same scope
        // snapshot. Keep consuming that local queue before paying for another full import pass.
        let mut needs_import_refresh = false;
        let mut next_scan_cursors = None;
        loop {
            if expansion_passes >= MAX_MACRO_EXPANSION_PASSES {
                // Stop expanding but still freeze a coherent def-map. The final import refresh lets
                // names generated before the cap settle into module scopes.
                mark_retryable_macros_skipped_by_limit(states);
                let timer = metric::TIMING_RESOLVE_IMPORT_SCOPES.start_timer();
                current_scopes = resolve_import_scopes(old, states)?;
                timer.finish();
                freeze_resolved_scopes(old, states, &current_scopes)?;
                return Ok(());
            }

            expansion_passes += 1;
            metric::EXPANSION_PASSES.inc();

            let timer = metric::TIMING_COLLECT_EXPANSION_ATTEMPTS.start_timer();
            let mut expansion_attempts = {
                let env = FinalizeResolutionEnv::new(old, states, &current_scopes);
                // The first pass in a round visits all pending macro calls. Follow-up passes only
                // visit calls appended by the previous expansion, because older unresolved calls
                // need a fresh import snapshot before their answer can change.
                let scan = next_scan_cursors
                    .as_ref()
                    .map(MacroExpansionScan::NewCallsSince)
                    .unwrap_or(MacroExpansionScan::AllPending);
                collect_expansion_attempts(&env, states, scan, &mut macro_cache)?
            };
            timer.finish();

            if expansion_attempts
                .iter()
                .any(|attempt| attempt.needs_expansion())
            {
                if macro_expansion_executor.is_none() {
                    macro_expansion_executor =
                        Some(MacroExpansionExecutor::new(performance_preference)?);
                }
                // The executor owns the rust-analyzer expansion adapter. It is created lazily so
                // projects without expandable declarative macros do not pay its setup cost.
                let executor = macro_expansion_executor
                    .as_ref()
                    .expect("macro expansion executor should be initialized");
                expand_expansion_attempts(executor, &mut expansion_attempts, &mut macro_cache);
            }

            let scan_cursors_before_apply = MacroExpansionCursors::capture(states);
            let timer = metric::TIMING_APPLY_EXPANSION_ATTEMPTS.start_timer();
            let expansion = if expansion_attempts.is_empty() {
                Default::default()
            } else {
                // Expanded text is parsed into regular item-tree data and appended to the owning
                // module. The same generated declarations are also added to `current_scopes`, which
                // makes simple chains like `make_macro!(); generated_macro!();` work in one round.
                apply_expansion_attempts(
                    item_tree,
                    states,
                    interners,
                    &mut current_scopes,
                    expansion_attempts,
                )?
            };
            timer.finish();

            needs_import_refresh |= expansion.changed;
            if expansion.changed {
                // Generated calls can be resolved with the same scope snapshot, but generated
                // imports cannot. Keep the cheap path going until no more direct expansion happens.
                next_scan_cursors = Some(scan_cursors_before_apply);
                continue;
            }

            if needs_import_refresh {
                // At least one expansion happened in this round. Re-run import resolution so
                // generated `use` items and newly exported names can participate in path lookup.
                break;
            }

            // No imports and no macros changed the visible declarations, so this is the stable
            // scope matrix that can be written into the frozen def maps.
            freeze_resolved_scopes(old, states, &current_scopes)?;
            return Ok(());
        }
    }
}

fn resolve_import_scopes(
    old: Option<&DefMapReadTxn<'_>>,
    states: &FinalizeTargetStates,
) -> anyhow::Result<ScopeMatrix> {
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
            return Ok(current_scopes);
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
            .def_map_builder
            .module_mut(ModuleId(module_idx))
            .expect("module should exist for every final dirty scope");
        module.scope = scope.freeze();
        module.unresolved_imports = final_unresolved_imports
            .get(module_idx)
            .expect("unresolved imports should exist for every dirty module")
            .clone();
    }
}

fn freeze_target_data(state: &TargetState) -> TargetData {
    // The same implicit roots used by import resolution are still needed by later frozen path
    // queries. Keep them as an extern prelude rather than pretending they are child modules of the
    // crate root.
    TargetData::new(
        Some(state.root_module),
        state.implicit_roots.clone(),
        state.prelude,
    )
}

fn freeze_target_state(state: &TargetState) -> DefMap {
    state.def_map_builder.clone().build()
}
