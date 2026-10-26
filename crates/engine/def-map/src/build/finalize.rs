//! Finalizes crate scopes into frozen def maps.
//!
//! Collection records direct declarations and raw imports, but leaves cross-crate facts unresolved.
//! This module turns those mutable crate states into immutable def maps by selecting preludes,
//! repeatedly applying imports and expanding item macros until scopes stop changing, and then
//! freezing the settled scopes into the crate payloads. Selected package reads come from fresh
//! crate states while other package reads fall through to the frozen baseline.
//!
//! Generated modules and includes can require more real source files. An injected loader grows
//! Parse and ItemTree between expansion batches while scopes, runtimes, and budgets stay on the
//! construction stack.

use std::sync::Arc;

use anyhow::Context as _;
use rg_ir_model::{
    CrateRef, DefId, DefMapRef, LocalDefRef, LocalEnumVariantRef, ModuleId, ModuleRef, PackageSlot,
    Path,
};
use rg_item_tree::ItemTreeDb;
use rg_macro_runtime::{MacroExpansionPerformancePreference, MacroExpansionRuntime};
use rg_parse::{Package, ParseDb};
use rg_std::UniqueVec;
use rg_text::{Name, PackageNameInterners};
use rg_workspace::{PackageOrigin, SysrootCrate, TargetKind, WorkspaceMetadata};

use super::{
    MacroSourceFileResolutions,
    collect::{CrateState, KnownModuleFiles},
    imports::{ImportResolutionExecutor, ImportWorklist},
    macros::{
        BuiltinDeriveExpansion, MAX_MACRO_EXPANSION_PASSES, MacroExpansionCursors,
        MacroExpansionScan, apply_expansion_attempts, apply_pending_macro_source_files,
        collect_expansion_attempts, expand_expansion_attempts,
        mark_retryable_macros_skipped_by_limit,
    },
};
use crate::{
    CrateData, CrateResolutionEnv, DefMapReadTxn, GeneratedItemStores, LocalDefData,
    LocalEnumVariantData, LocalEnumVariantEntry, MacroDefinitionEnv, MacroDefinitionView,
    MacroExpansionLimitReport, MacroSourceFileLoader, MacroSourceFileRequest,
    MacroSourceFileResolution, ModuleData, ModuleScopeBuilder, Namespace,
    PackageDefMaps as DefMapPackage, ScopeBindingProvenance, ScopeEntryRef, ScopeResolutionEnv,
    ScopeResolver,
    profile::{ImportResolutionPassMetrics, metric, record_import_resolution_pass},
};

/// Mutable crate states for every crate inside one package.
pub(super) type PackageCrateStates = Vec<CrateState>;

/// Collected crate states that must be finalized.
///
/// `Some` package slots are being rebuilt and own fresh crate state. `None` slots are left unchanged;
/// resolution reads their declarations and scopes from the caller's frozen baseline.
pub(super) struct FinalizeCrateStates {
    packages: Vec<Option<PackageCrateStates>>,
}

impl FinalizeCrateStates {
    pub(super) fn empty(package_count: usize) -> Self {
        Self {
            packages: (0..package_count).map(|_| None).collect(),
        }
    }

    pub(super) fn replace_package(
        &mut self,
        package: PackageSlot,
        states: Vec<CrateState>,
    ) -> Option<()> {
        *self.packages.get_mut(package.0)? = Some(states);
        Some(())
    }

    pub(super) fn package(&self, package: PackageSlot) -> Option<&[CrateState]> {
        self.packages.get(package.0)?.as_deref()
    }

    pub(super) fn take_package(&mut self, package: PackageSlot) -> Option<Vec<CrateState>> {
        self.packages.get_mut(package.0)?.take()
    }

    pub(super) fn crate_state(&self, crate_ref: CrateRef) -> Option<&CrateState> {
        self.package(crate_ref.package)?.get(crate_ref.crate_id.0)
    }

    pub(super) fn crate_state_mut(&mut self, crate_ref: CrateRef) -> Option<&mut CrateState> {
        self.packages
            .get_mut(crate_ref.package.0)?
            .as_deref_mut()?
            .get_mut(crate_ref.crate_id.0)
    }

    pub(super) fn iter_dirty(&self) -> impl Iterator<Item = &[CrateState]> {
        self.packages.iter().filter_map(Option::as_deref)
    }

    pub(super) fn iter_dirty_mut(&mut self) -> impl Iterator<Item = &mut [CrateState]> {
        self.packages.iter_mut().filter_map(Option::as_deref_mut)
    }

    pub(super) fn iter_dirty_mut_enumerated(
        &mut self,
    ) -> impl Iterator<Item = (usize, &mut [CrateState])> {
        self.packages
            .iter_mut()
            .enumerate()
            .filter_map(|(package_slot, states)| {
                states.as_deref_mut().map(|states| (package_slot, states))
            })
    }

    /// Clears the source batch after the loader has supplied its results.
    pub(super) fn clear_macro_source_file_requests(&mut self) {
        for package_states in self.iter_dirty_mut() {
            for state in package_states {
                state.macro_source_file_requests.clear();
            }
        }
    }

    /// Refreshes known package files after late Parse/ItemTree growth.
    pub(super) fn refresh_known_module_files(
        &mut self,
        packages: &[Package],
        item_tree: &ItemTreeDb,
    ) {
        for (package_slot, package_states) in self.iter_dirty_mut_enumerated() {
            let item_tree_package = item_tree
                .package(package_slot)
                .expect("dirty package should have an item tree while finalizing");
            let known_module_files = Arc::new(KnownModuleFiles::from_package(
                &packages[package_slot],
                item_tree_package,
            ));
            for state in package_states {
                state.known_module_files = Arc::clone(&known_module_files);
            }
        }
    }

    /// Coalesces construction requests without retaining them in frozen package payloads.
    pub(super) fn macro_source_file_requests(&self) -> Vec<MacroSourceFileRequest> {
        let mut requests = UniqueVec::new();

        for package_states in self.iter_dirty() {
            for state in package_states {
                for request in &state.macro_source_file_requests {
                    requests.push(request.clone());
                }
            }
        }

        requests.into_vec()
    }

    pub(super) fn module_scope(&self, module: ModuleRef) -> Option<&ModuleScopeBuilder> {
        self.crate_state(module.origin.as_crate_ref()?)?
            .def_map_builder
            .module_scope(module.module)
    }

    /// Keep proc-macro implementation items private before another crate can import them.
    fn censor_proc_macro_exports(&mut self) {
        for package_states in self.iter_dirty_mut() {
            for state in package_states {
                if state.target_kind != TargetKind::ProcMacro {
                    continue;
                }
                // Censoring reads declaration metadata while changing the resolved scope. Move the
                // scope out briefly so those borrows stay separate without cloning its bindings.
                let root = state.root_module;
                let mut scope = std::mem::take(
                    state
                        .def_map_builder
                        .module_scope_mut(root)
                        .expect("proc-macro root scope should exist"),
                );
                state.censor_proc_macro_scope(root, &mut scope);
                *state
                    .def_map_builder
                    .module_scope_mut(root)
                    .expect("proc-macro root scope should remain allocated") = scope;
            }
        }
    }
}

impl CrateState {
    /// Make one rebuilt scope obey the external export rules of a proc-macro crate.
    ///
    /// A function marked `#[proc_macro]` is a normal value inside its own source crate, but another
    /// crate may see only its macro identity. Worklist candidates call this before comparison so
    /// the hidden function value cannot look like a scope change and wake more import groups.
    pub(super) fn censor_proc_macro_scope(&self, module: ModuleId, scope: &mut ModuleScopeBuilder) {
        if self.target_kind != TargetKind::ProcMacro || module != self.root_module {
            return;
        }

        let root = ModuleRef::krate(self.crate_ref, self.root_module);
        let def_map = self.def_map_builder.partial();
        scope.censor_public_bindings(root, |namespace, binding| {
            if namespace != Namespace::Macros
                || !binding
                    .routes()
                    .iter()
                    .any(|route| route.provenance == ScopeBindingProvenance::Direct)
            {
                return false;
            }

            let DefId::Local(local_def) = binding.def else {
                return false;
            };
            local_def.origin == root.origin
                && def_map
                    .macro_definition(local_def.local_def)
                    .and_then(|data| data.proc_macro_implementation())
                    .is_some()
        });
    }
}

/// Resolution environment used while dirty package scopes are being fixed up.
///
/// Dirty package reads come from fresh crate state and the current fixed-point module scopes.
/// Clean package reads fall through to the frozen baseline.
///
/// For dirty crates, `PartialDefMap` exposes module metadata but its `ModuleData::scope` is not
/// filled yet. Their scope queries read the builder's resolved scope instead. Borrowing all states
/// immutably keeps those scopes unchanged while import workers build their candidates.
pub(super) struct FinalizeResolutionEnv<'a> {
    old: &'a DefMapReadTxn<'a>,
    states: &'a FinalizeCrateStates,
}

impl<'a> FinalizeResolutionEnv<'a> {
    fn new(old: &'a DefMapReadTxn<'a>, states: &'a FinalizeCrateStates) -> Self {
        Self { old, states }
    }

    /// Return the input scope a module worker uses to decide whether its candidate changed.
    pub(super) fn current_module_scope(&self, module: ModuleRef) -> Option<&ModuleScopeBuilder> {
        self.states.module_scope(module)
    }
}

impl ScopeResolutionEnv for FinalizeResolutionEnv<'_> {
    type Error = rg_package_store::PackageStoreError;

    fn module_data(
        &self,
        module_ref: ModuleRef,
    ) -> Result<Option<&ModuleData>, rg_package_store::PackageStoreError> {
        if let Some(crate_ref) = module_ref.origin.as_crate_ref()
            && let Some(state) = self.states.crate_state(crate_ref)
        {
            return Ok(state.def_map_builder.partial().module(module_ref.module));
        }

        let Some(crate_ref) = module_ref.origin.as_crate_ref() else {
            return Ok(None);
        };
        Ok(self
            .old
            .def_map(crate_ref)?
            .and_then(|def_map| def_map.module(module_ref.module)))
    }

    fn module_scope_entry<'a>(
        &'a self,
        module_ref: ModuleRef,
        name: &str,
    ) -> Result<Option<ScopeEntryRef<'a>>, rg_package_store::PackageStoreError> {
        if module_ref
            .origin
            .as_crate_ref()
            .is_some_and(|crate_ref| self.states.package(crate_ref.package).is_some())
        {
            return Ok(self
                .states
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
            .as_crate_ref()
            .is_some_and(|crate_ref| self.states.package(crate_ref.package).is_some())
        {
            return Ok(self
                .states
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
        if let Some(crate_ref) = local_def_ref.origin.as_crate_ref()
            && let Some(state) = self.states.crate_state(crate_ref)
        {
            return Ok(state
                .def_map_builder
                .partial()
                .local_def(local_def_ref.local_def));
        }

        let Some(crate_ref) = local_def_ref.origin.as_crate_ref() else {
            return Ok(None);
        };
        Ok(self
            .old
            .def_map(crate_ref)?
            .and_then(|def_map| def_map.local_def(local_def_ref.local_def)))
    }

    fn local_enum_variant_data(
        &self,
        variant_ref: LocalEnumVariantRef,
    ) -> Result<Option<&LocalEnumVariantData>, rg_package_store::PackageStoreError> {
        if let Some(crate_ref) = variant_ref.origin.as_crate_ref()
            && let Some(state) = self.states.crate_state(crate_ref)
        {
            return Ok(state
                .def_map_builder
                .partial()
                .local_enum_variant(variant_ref.local_enum_variant));
        }

        let Some(crate_ref) = variant_ref.origin.as_crate_ref() else {
            return Ok(None);
        };
        Ok(self
            .old
            .def_map(crate_ref)?
            .and_then(|def_map| def_map.local_enum_variant(variant_ref.local_enum_variant)))
    }

    fn local_enum_variant_entries_for_enum<'a>(
        &'a self,
        enum_def: LocalDefRef,
    ) -> Result<Vec<LocalEnumVariantEntry<'a>>, rg_package_store::PackageStoreError> {
        if let Some(crate_ref) = enum_def.origin.as_crate_ref()
            && let Some(state) = self.states.crate_state(crate_ref)
        {
            return Ok(state
                .def_map_builder
                .partial()
                .local_enum_variant_entries_for_enum(enum_def.local_def)
                .collect());
        }

        if let Some(crate_ref) = enum_def.origin.as_crate_ref()
            && let Some(def_map) = self.old.def_map(crate_ref)?
        {
            Ok(def_map
                .local_enum_variant_entries_for_enum(enum_def.local_def)
                .collect())
        } else {
            Ok(Vec::new())
        }
    }
}

impl MacroDefinitionEnv for FinalizeResolutionEnv<'_> {
    fn macro_definition_view<'a>(
        &'a self,
        def: DefId,
    ) -> Result<Option<MacroDefinitionView<'a>>, rg_package_store::PackageStoreError> {
        let DefId::Local(def_ref) = def else {
            return Ok(None);
        };
        let Some(local_def) = self.local_def_data(def_ref)? else {
            return Ok(None);
        };

        let data = if let Some(crate_ref) = def_ref.origin.as_crate_ref()
            && let Some(state) = self.states.crate_state(crate_ref)
        {
            state
                .def_map_builder
                .partial()
                .macro_definition(def_ref.local_def)
        } else {
            let Some(crate_ref) = def_ref.origin.as_crate_ref() else {
                return Ok(None);
            };
            self.old
                .def_map(crate_ref)?
                .and_then(|def_map| def_map.macro_definition(def_ref.local_def))
        };
        let Some(data) = data else {
            return Ok(None);
        };

        Ok(MacroDefinitionView::new(def_ref, local_def, data))
    }
}

impl CrateResolutionEnv for FinalizeResolutionEnv<'_> {
    fn extern_root(
        &self,
        crate_ref: CrateRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, rg_package_store::PackageStoreError> {
        if let Some(state) = self.states.crate_state(crate_ref) {
            return Ok(state.extern_prelude.resolve(name));
        }

        Ok(self
            .old
            .package(crate_ref.package)?
            .crate_data(crate_ref.crate_id)
            .and_then(|data| data.extern_prelude().get(name).copied()))
    }

    fn prelude_module(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Option<ModuleRef>, rg_package_store::PackageStoreError> {
        if let Some(state) = self.states.crate_state(crate_ref) {
            return Ok(state.prelude);
        }

        Ok(self
            .old
            .package(crate_ref.package)?
            .crate_data(crate_ref.crate_id)
            .and_then(|data| data.prelude()))
    }

    fn root_module(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Option<ModuleRef>, rg_package_store::PackageStoreError> {
        let module = if let Some(state) = self.states.crate_state(crate_ref) {
            Some(state.root_module)
        } else {
            self.old
                .package(crate_ref.package)?
                .crate_data(crate_ref.crate_id)
                .and_then(|data| data.root_module())
        };

        Ok(module.map(|module| ModuleRef {
            origin: DefMapRef::Crate(crate_ref),
            module,
        }))
    }
}

/// Selects the standard prelude module visible from each dirty crate.
///
/// The prelude path depends on the crate_ref edition, and the module it resolves to can live in a
/// unselected package. Resolution therefore uses the same fresh-state-plus-baseline environment
/// as the later import fixed point.
pub(super) fn select_preludes(
    old: &DefMapReadTxn<'_>,
    workspace: &WorkspaceMetadata,
    packages: &[Package],
    states: &mut FinalizeCrateStates,
    interners: &mut PackageNameInterners,
) -> anyhow::Result<()> {
    // No imports have run yet, so module scopes contain only direct declarations. Apply the
    // proc-macro export boundary before looking up preludes across crates.
    states.censor_proc_macro_exports();
    let env = FinalizeResolutionEnv::new(old, states);

    // Store selected preludes out-of-band first so path resolution can borrow all crate states
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
        // Each crate resolves its edition prelude from its own root. Crates without a root module
        // are malformed enough that later phases will simply see no prelude.
        for (crate_slot, state) in package_states.iter().enumerate() {
            let mut prelude_module = None;

            // The sysroot's std and core crates provide their own preludes, even without an
            // `extern crate self as std` or `core` alias. Resolve their local prelude first:
            // falling back to core inside std would leave types such as Box unresolved.
            // Other crates prefer std's edition prelude, falling back to core when unavailable.
            // TODO: Parse crate-level `#![no_std]` and use it to select `core` prelude directly
            // and avoid exposing `std` as an automatic extern root for that crate.
            let prelude_paths = [
                matches!(
                    workspace_package.origin,
                    PackageOrigin::Sysroot(SysrootCrate::Std | SysrootCrate::Core)
                )
                .then(|| {
                    Path::crate_relative_standard_prelude(workspace_package.edition, interner)
                }),
                Some(Path::standard_prelude(
                    "std",
                    workspace_package.edition,
                    interner,
                )),
                Some(Path::standard_prelude(
                    "core",
                    workspace_package.edition,
                    interner,
                )),
            ];

            for prelude_path in prelude_paths.into_iter().flatten() {
                let root_module = ModuleRef::krate(state.crate_ref, state.root_module);
                prelude_module = ScopeResolver::new(&env)
                    .import_modules(root_module, &prelude_path)?
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
            package_preludes[crate_slot] = Some(prelude_module);
        }
    }

    // Apply the selected modules after lookup is done so future import resolution can consult the
    // prelude through `CrateResolutionEnv::prelude_module`.
    for (package_slot, package_states) in states.iter_dirty_mut_enumerated() {
        let package_preludes = selected_preludes[package_slot]
            .as_ref()
            .expect("prelude slots should exist for every dirty package");
        for (crate_slot, state) in package_states.iter_mut().enumerate() {
            state.prelude = package_preludes[crate_slot];
        }
    }

    Ok(())
}

/// Resolves imports and item-position macros until every dirty crate scope stops changing.
///
/// Imports can depend on names introduced by other imports, and macro calls can depend on imports
/// that make the macro definition visible. This function therefore runs a small fixed-point loop:
/// resolve imports against the current crate states, expand the macros that are now visible,
/// splice generated items back into the mutable crate states, and refresh imports whenever those
/// generated items may have introduced new imports or exported names.
///
/// Source discovery happens inside the requesting macro batch. After loading, collect the real
/// module/include into the same scopes and continue calls appended since before that batch. Import
/// refresh waits until that queue drains, just as it does for directly generated declarations.
pub(super) fn finalize_scopes(
    old: &DefMapReadTxn<'_>,
    parse: &mut ParseDb,
    item_tree: &mut ItemTreeDb,
    states: &mut FinalizeCrateStates,
    interners: &mut PackageNameInterners,
    performance_preference: MacroExpansionPerformancePreference,
    source_loader: &mut impl MacroSourceFileLoader,
) -> anyhow::Result<()> {
    metric::EXPANSION_PASS_LIMIT.record_count(MAX_MACRO_EXPANSION_PASSES);

    // One call owns the complete fixed point. Loading files cannot reset the expansion budget or
    // discard compiled macros, import workers, or the scopes already produced by this build.
    let mut macro_runtime = MacroExpansionRuntime::new(performance_preference);
    let mut import_executor = ImportResolutionExecutor::new(performance_preference);
    let mut import_resolution_runs = 0;
    let mut expansion_passes = 0;
    let mut macro_source_file_resolutions = MacroSourceFileResolutions::default();

    loop {
        metric::ROUNDS.inc();
        let timer = metric::TIMING_RESOLVE_IMPORT_SCOPES.start_timer();
        resolve_import_scopes(
            old,
            states,
            &mut import_executor,
            &mut import_resolution_runs,
        )?;
        timer.finish();
        let mut next_scan_cursors = None;
        // Direct additions are visible immediately; dependent imports and their reports need replay.
        let mut needs_import_refresh = false;

        // Macro expansion can introduce more macro calls visible through direct bindings. Keep
        // consuming that local queue before paying for another full import pass.
        loop {
            if expansion_passes >= MAX_MACRO_EXPANSION_PASSES {
                // Stop expanding but still freeze a coherent def-map. The final import refresh lets
                // names generated before the cap settle into module scopes.
                mark_retryable_macros_skipped_by_limit(states);
                let timer = metric::TIMING_RESOLVE_IMPORT_SCOPES.start_timer();
                resolve_import_scopes(
                    old,
                    states,
                    &mut import_executor,
                    &mut import_resolution_runs,
                )?;
                timer.finish();
                lower_builtin_derives(old, item_tree, states, interners)?;
                return Ok(());
            }

            expansion_passes += 1;
            metric::EXPANSION_PASSES.inc();

            let timer = metric::TIMING_COLLECT_EXPANSION_ATTEMPTS.start_timer();
            let mut expansion_attempts = {
                let env = FinalizeResolutionEnv::new(old, states);
                // The first pass in a round visits all pending macro calls. Follow-up passes only
                // visit calls appended by the previous expansion, because older unresolved calls
                // need a fresh import snapshot before their answer can change.
                let scan = next_scan_cursors
                    .as_ref()
                    .map(MacroExpansionScan::NewCallsSince)
                    .unwrap_or(MacroExpansionScan::AllPending);
                collect_expansion_attempts(
                    &env,
                    states,
                    scan,
                    &mut macro_runtime,
                    &macro_source_file_resolutions,
                )?
            };
            timer.finish();

            if expansion_attempts
                .iter()
                .any(|attempt| attempt.needs_expansion())
            {
                // The runtime owns the worker pool and creates it lazily, so projects without
                // expandable declarative macros do not pay its setup cost.
                expand_expansion_attempts(&mut macro_runtime, &mut expansion_attempts)?;
            }

            let scan_cursors_before_apply = MacroExpansionCursors::capture(states);
            let timer = metric::TIMING_APPLY_EXPANSION_ATTEMPTS.start_timer();
            let expansion = if expansion_attempts.is_empty() {
                Default::default()
            } else {
                // Expanded text is parsed into regular item-tree data and appended to the owning
                // module. Direct bindings also enter its resolved scope, so simple chains like
                // `make_macro!(); generated_macro!();` work in one round.
                apply_expansion_attempts(
                    item_tree,
                    states,
                    interners,
                    expansion_attempts,
                    &macro_source_file_resolutions,
                )?
            };
            timer.finish();

            needs_import_refresh |= expansion.changed;
            if expansion.changed {
                states.censor_proc_macro_exports();
            }

            // All lookup and item borrows have ended before the loader can grow Parse/ItemTree.
            // Complete this batch before collecting real source into the requesting scopes.
            let requests = states.macro_source_file_requests();
            if !requests.is_empty() {
                let resolutions = source_loader
                    .load_files(parse, item_tree, interners, &requests)
                    .context("load macro source files")?;
                anyhow::ensure!(
                    resolutions.len() == requests.len(),
                    "macro source loader must return one result per request",
                );
                for (request, resolution) in requests.into_iter().zip(resolutions) {
                    anyhow::ensure!(
                        matches!(
                            (&request, &resolution),
                            (_, MacroSourceFileResolution::Missing)
                                | (
                                    MacroSourceFileRequest::Module { .. },
                                    MacroSourceFileResolution::Module { .. }
                                )
                                | (
                                    MacroSourceFileRequest::Include { .. },
                                    MacroSourceFileResolution::Include { .. }
                                )
                        ),
                        "macro source loader returned the wrong file kind for {}",
                        request.description(),
                    );
                    macro_source_file_resolutions.insert(request, resolution);
                }
                states.clear_macro_source_file_requests();
                states.refresh_known_module_files(parse.packages(), item_tree);
                apply_pending_macro_source_files(
                    item_tree,
                    states,
                    &macro_source_file_resolutions,
                )?;
                states.censor_proc_macro_exports();

                // Even a missing result belongs to a batch that may have added other declarations.
                // Drain both its generated calls and the newly loaded calls before refreshing
                // imports, then require matching import reports before anything can freeze.
                needs_import_refresh = true;
                next_scan_cursors = Some(scan_cursors_before_apply);
                continue;
            }

            if expansion.changed {
                // Generated calls can be resolved with the same module scopes, but generated
                // imports cannot. Keep the cheap path going until no more direct expansion happens.
                next_scan_cursors = Some(scan_cursors_before_apply);
                continue;
            }

            if needs_import_refresh {
                // At least one expansion happened in this round. Re-run import resolution so
                // generated `use` items and newly exported names can participate in path lookup.
                break;
            }

            // No imports or macros changed the visible declarations. Module builders now own the
            // settled scopes and reports; only builtin impl synthesis remains before freezing.
            lower_builtin_derives(old, item_tree, states, interners)?;
            return Ok(());
        }
    }
}

/// Settle import-derived bindings without rebuilding every import group on every wave.
///
/// The worklist starts with every module that owns imports. Each wave rebuilds only the active
/// modules against one shared scope snapshot, installs all changed candidates together, and wakes
/// groups which read a changed scope. For a chain such as `root -> bridge -> source`, this lets
/// `bridge` gain a binding first and wakes `root` for the following wave.
///
/// When no changed scope has a reader, both the scopes and their latest unresolved-import reports
/// are ready to freeze.
fn resolve_import_scopes(
    old: &DefMapReadTxn<'_>,
    states: &mut FinalizeCrateStates,
    import_executor: &mut ImportResolutionExecutor,
    import_resolution_runs: &mut usize,
) -> anyhow::Result<()> {
    metric::IMPORT_RESOLUTION_RUNS.inc();
    *import_resolution_runs += 1;
    let run_index = *import_resolution_runs;
    let mut pass_index = 0;
    let mut worklist = ImportWorklist::new(states);

    while !worklist.active_modules().is_empty() {
        pass_index += 1;
        metric::IMPORT_RESOLUTION_PASSES.inc();

        // Finish the active wave before changing module scopes. If `root` imports from `bridge`
        // and `bridge` imports from `source`, `root` must not see `bridge`'s new binding until the
        // next wave; otherwise the result would depend on which worker happened to finish first.
        let updates = {
            let env = FinalizeResolutionEnv::new(old, states);
            import_executor
                .apply_wave(&worklist, states, &env)
                .context("while attempting to resolve import wave")?
        };
        let outcome = worklist.apply_updates(states, updates);
        record_import_resolution_pass(
            run_index,
            pass_index,
            ImportResolutionPassMetrics {
                evaluated_packages: outcome.evaluated.packages,
                evaluated_crates: outcome.evaluated.crates,
                evaluated_modules: outcome.evaluated.modules,
                changed_packages: outcome.changed.packages,
                changed_crates: outcome.changed.crates,
                changed_modules: outcome.changed.modules,
                imports_evaluated: outcome.stats.imports_evaluated,
                glob_imports_evaluated: outcome.stats.glob_imports_evaluated,
                glob_bindings_emitted: outcome.stats.glob_bindings_emitted,
            },
        );

        if outcome.finished {
            break;
        }
    }

    Ok(())
}

fn lower_builtin_derives(
    old: &DefMapReadTxn<'_>,
    item_tree: &ItemTreeDb,
    states: &mut FinalizeCrateStates,
    interners: &mut PackageNameInterners,
) -> anyhow::Result<()> {
    // Builtin derives add impls, but no imports or module bindings. Once normal expansion has
    // settled, synthesize them once, including derives on declarations produced by macros.
    let derives = BuiltinDeriveExpansion::collect(
        &FinalizeResolutionEnv::new(old, states),
        states,
        item_tree,
        interners,
    )
    .context("lower builtin derive impls")?;
    for derive in derives {
        derive.apply(states);
    }
    Ok(())
}

/// Freezes collected crate states into the package payload stored by `DefMapDb`.
pub(super) fn freeze_package(
    package: &Package,
    package_states: Vec<CrateState>,
    generated_items: &mut GeneratedItemStores,
) -> DefMapPackage {
    let package_name = package.package_name().to_string();
    let mut crates = Vec::with_capacity(package_states.len());
    let mut macro_expansion_limits = Vec::new();

    // Split each builder by moving its frozen DefMap into the package and its generated syntax into
    // the transient phase output. This avoids both cloning the expansion arena and materializing an
    // intermediate collection that the caller would immediately consume.
    for state in package_states {
        if let Some(report) = &state.macro_expansion_limit {
            macro_expansion_limits.push(MacroExpansionLimitReport {
                package_name: package_name.clone(),
                crate_name: state.crate_name.clone(),
                groups: report.groups.clone(),
                omitted_call_count: report.omitted_call_count,
            });
        }

        let crate_ref = state.crate_ref;
        let (def_map, crate_generated_items) = state.def_map_builder.into_parts();
        generated_items.insert(crate_ref, crate_generated_items);

        // Const arithmetic also needs the compilation target's usize width after parsed package
        // data is offloaded. Missing target information must not fall back to the host width.
        let target_pointer_width = state
            .cfg_options
            .key_values()
            .iter()
            .find(|value| value.key() == "target_pointer_width")
            .and_then(|value| value.value().parse().ok());

        // Persist both Cargo-provided roots and explicit crate-root aliases. Queries read this as a
        // prelude rather than pretending any of these names are child modules of the crate root.
        crates.push(CrateData::new(
            state.cargo_target,
            state.target_kind,
            target_pointer_width,
            state.crate_name,
            Some(state.root_module),
            state.extern_prelude.freeze(),
            state.prelude,
            def_map,
        ));
    }

    DefMapPackage::new(
        package_name,
        package.edition(),
        crates,
        macro_expansion_limits,
    )
}
