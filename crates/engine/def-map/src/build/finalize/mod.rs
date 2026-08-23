//! Finalizes crate scopes into frozen def maps.
//!
//! Collection records direct declarations and raw imports, but it intentionally leaves cross-crate
//! facts unresolved. This module turns those mutable crate states into immutable def maps by
//! selecting preludes, repeatedly applying imports until scopes stop changing, and then freezing
//! the settled scopes back into the crate payloads. During package rebuilds, dirty package reads
//! come from fresh crate states while clean package reads fall through to the old frozen database.
//!
//! Resumable package finalization can pause after a generated macro asks for an out-of-line module
//! file. The project grows Parse and ItemTree, then calls this fixed-point machinery again with the
//! source resolution; the macro runtime, expansion budget, mutable crate states, and settled scopes
//! survive the pause.

mod clean;
mod rebuild;

use std::sync::Arc;

use anyhow::Context as _;
use rayon::prelude::*;

use crate::{
    CrateData, CrateResolutionEnv, LocalDefData, LocalEnumVariantData, LocalEnumVariantEntry,
    MacroDefinitionEnv, MacroDefinitionView, MacroExpansionLimitReport, ModuleData,
    ModuleScopeBuilder, Namespace, PackageDefMaps as DefMapPackage, ScopeBindingProvenance,
    ScopeEntryRef, ScopeResolutionEnv, ScopeResolver,
};
use rg_ir_model::{
    CrateRef, DefId, DefMapRef, LocalDefRef, LocalEnumVariantRef, ModuleId, ModuleRef, Path,
};
use rg_item_tree::ItemTreeDb;
use rg_macro_runtime::{MacroExpansionPerformancePreference, MacroExpansionRuntime};
use rg_parse::Package;
use rg_std::UniqueVec;
use rg_text::{Name, PackageNameInterners};
use rg_workspace::{TargetKind, WorkspaceMetadata};

use crate::{DefMapReadTxn, GeneratedModuleRequest, PackageSlot, profile::metric};

use super::GeneratedModuleResolutions;

use super::{
    collect::{CrateState, KnownModuleFiles},
    imports::{UnresolvedImports, apply_imports},
    macros::{
        MAX_MACRO_EXPANSION_PASSES, MacroExpansionCursors, MacroExpansionScan,
        apply_expansion_attempts, apply_pending_generated_modules, collect_expansion_attempts,
        expand_expansion_attempts, mark_retryable_macros_skipped_by_limit,
    },
};

pub use self::rebuild::DefMapRebuildSession;
pub(super) use self::rebuild::start_package_build_session;
pub(crate) use self::{clean::build_db, rebuild::rebuild_packages};

/// Mutable crate states for every crate inside one package.
pub(super) type PackageCrateStates = Vec<CrateState>;

/// Mutable module scopes for one crate.
type CrateScopeMatrix = Vec<ModuleScopeBuilder>;

/// Mutable module scopes for every crate inside one package.
type PackageScopeMatrix = Vec<CrateScopeMatrix>;

// Below this size, dispatching packages through a worker pool costs more than the import work.
const PARALLEL_IMPORT_RESOLUTION_PACKAGE_THRESHOLD: usize = 32;
const LOWER_PEAK_MEMORY_IMPORT_RESOLUTION_THREAD_LIMIT: usize = 2;

/// Collected crate states that must be finalized.
///
/// `Some` package slots are dirty and will be resolved/frozen. `None` slots are only valid when an
/// old `DefMapDb` baseline exists; resolution reads them from that frozen baseline instead.
pub(super) struct FinalizeCrateStates {
    packages: Vec<Option<PackageCrateStates>>,
}

impl FinalizeCrateStates {
    pub(super) fn all(packages: Vec<PackageCrateStates>) -> Self {
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
        states: Vec<CrateState>,
    ) -> Option<()> {
        *self.packages.get_mut(package.0)? = Some(states);
        Some(())
    }

    pub(super) fn take_package(&mut self, package: PackageSlot) -> Option<Vec<CrateState>> {
        self.packages.get_mut(package.0)?.take()
    }

    pub(super) fn package(&self, package: PackageSlot) -> Option<&[CrateState]> {
        self.packages.get(package.0)?.as_deref()
    }

    pub(super) fn iter_packages(&self) -> impl Iterator<Item = Option<&[CrateState]>> + '_ {
        self.packages.iter().map(Option::as_deref)
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

    fn base_scopes(&self) -> ScopeMatrix {
        ScopeMatrix::from_crate_states(self)
    }

    /// Clears requests emitted by the previous resumable construction step.
    pub(super) fn clear_generated_module_requests(&mut self) {
        for package_states in self.iter_dirty_mut() {
            for state in package_states {
                state.generated_module_requests.clear();
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
    fn generated_module_requests(&self) -> Vec<GeneratedModuleRequest> {
        let mut requests = UniqueVec::new();

        for package_states in self.iter_dirty() {
            for state in package_states {
                for request in &state.generated_module_requests {
                    requests.push(request.clone());
                }
            }
        }

        requests.into_vec()
    }

    /// Prevents one-shot builders from freezing unresolved generated module continuations.
    pub(super) fn ensure_no_generated_module_requests(&self) -> anyhow::Result<()> {
        let requests = self.generated_module_requests();
        anyhow::ensure!(
            requests.is_empty(),
            "DefMap construction requires {} generated out-of-line module source request(s); use the project-owned resumable build path",
            requests.len(),
        );
        Ok(())
    }
}

/// Fixed-point machinery retained across project-owned source discovery pauses.
///
/// The expansion pass budget, macro runtime, and exact macro-worklist continuation survive every
/// pause. Late source discovery therefore cannot reset the recursion guard, recompile macros
/// already handled before a file request was emitted, or restart global import resolution merely
/// because the project had to capture another source file.
pub(super) struct FinalizeScopeSession {
    macro_runtime: MacroExpansionRuntime,
    import_executor: ImportResolutionExecutor,
    expansion_passes: usize,
    continuation: Option<MacroExpansionContinuation>,
}

/// The point immediately after an expansion batch requested project-owned source files.
///
/// The project may need several sequential source waves before the macro queue drains. Keeping the
/// scopes and scan cursor from the requesting batch lets each loaded module join that same queue;
/// imports are refreshed only after the cheap expansion chain can no longer continue.
struct MacroExpansionContinuation {
    current_scopes: ScopeMatrix,
    next_scan_cursors: MacroExpansionCursors,
}

impl FinalizeScopeSession {
    pub(super) fn new(performance_preference: MacroExpansionPerformancePreference) -> Self {
        Self {
            macro_runtime: MacroExpansionRuntime::new(performance_preference),
            import_executor: ImportResolutionExecutor::new(performance_preference),
            expansion_passes: 0,
            continuation: None,
        }
    }
}

/// Import-resolution scopes for dirty packages.
///
/// The axes are package slot, crate id, then module id. Clean package slots are absent and read
/// from the optional frozen baseline instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ScopeMatrix {
    packages: Vec<Option<PackageScopeMatrix>>,
}

impl ScopeMatrix {
    fn from_crate_states(states: &FinalizeCrateStates) -> Self {
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

        let mut scopes = Self { packages };
        scopes.censor_proc_macro_exports(states);
        scopes
    }

    fn crate_scopes(&self, crate_ref: CrateRef) -> Option<&[ModuleScopeBuilder]> {
        self.packages
            .get(crate_ref.package.0)?
            .as_ref()?
            .get(crate_ref.crate_id.0)
            .map(Vec::as_slice)
    }

    fn module_scope(&self, module: ModuleRef) -> Option<&ModuleScopeBuilder> {
        self.crate_scopes(module.origin.as_crate_ref()?)?
            .get(module.module.0)
    }

    pub(super) fn module_scope_mut(
        &mut self,
        crate_ref: CrateRef,
        module: ModuleId,
    ) -> Option<&mut ModuleScopeBuilder> {
        self.packages
            .get_mut(crate_ref.package.0)?
            .as_mut()?
            .get_mut(crate_ref.crate_id.0)?
            .get_mut(module.0)
    }

    pub(super) fn push_module_scope(
        &mut self,
        crate_ref: CrateRef,
        scope: ModuleScopeBuilder,
    ) -> Option<()> {
        self.packages
            .get_mut(crate_ref.package.0)?
            .as_mut()?
            .get_mut(crate_ref.crate_id.0)?
            .push(scope);
        Some(())
    }

    /// Apply the language-level export surface of every dirty proc-macro target.
    ///
    /// ```text
    /// #[proc_macro]
    /// pub fn emit(input: TokenStream) -> TokenStream { /* ... */ }
    ///
    /// pub fn helper() {}
    /// ```
    ///
    /// Cargo still gives the implementation functions normal source visibility inside their own
    /// crate. Across the crate boundary, however, only directly declared proc-macro identities are
    /// exported: downstream code can use `emit!`, but cannot import the value `emit` or `helper`.
    ///
    /// Import and macro expansion are fixed-point operations. Applying the censoring to every
    /// mutable scope snapshot prevents a public re-export or generated item from being observed by
    /// another crate during an intermediate pass and surviving in its settled imports.
    fn censor_proc_macro_exports(&mut self, states: &FinalizeCrateStates) {
        for package_states in states.iter_dirty() {
            for state in package_states {
                if state.target_kind != TargetKind::ProcMacro {
                    continue;
                }

                let root = ModuleRef::krate(state.crate_ref, state.root_module);
                let def_map = state.def_map_builder.partial();
                self.module_scope_mut(state.crate_ref, state.root_module)
                    .expect("proc-macro root scope should exist")
                    .censor_public_bindings(root, |namespace, binding| {
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
    }
}

/// Owns the worker pool used to apply imports for large package sets.
///
/// One fixed-point pass reads only from the immutable previous `ScopeMatrix`, while each dirty
/// package writes to its own part of the next matrix. That makes package-level work independent;
/// fixed-point passes themselves remain serial so later passes observe a complete snapshot. The
/// pool is local to finalization, so its worker threads do not become retained LSP state.
struct ImportResolutionExecutor {
    performance_preference: MacroExpansionPerformancePreference,
    thread_pool: Option<rayon::ThreadPool>,
}

impl ImportResolutionExecutor {
    fn new(performance_preference: MacroExpansionPerformancePreference) -> Self {
        Self {
            performance_preference,
            thread_pool: None,
        }
    }

    /// Apply one complete import pass, using parallel package jobs only when there is enough work.
    fn apply_pass(
        &mut self,
        states: &FinalizeCrateStates,
        env: &FinalizeResolutionEnv<'_>,
        next_scopes: &mut ScopeMatrix,
    ) -> anyhow::Result<()> {
        let apply_package = |(next_package, package_states): (
            &mut Option<PackageScopeMatrix>,
            &Option<PackageCrateStates>,
        )|
         -> anyhow::Result<()> {
            let (Some(next_package), Some(package_states)) =
                (next_package.as_mut(), package_states.as_ref())
            else {
                assert!(
                    next_package.is_none() && package_states.is_none(),
                    "scope and crate-state package slots should match"
                );
                return Ok(());
            };
            assert_eq!(
                next_package.len(),
                package_states.len(),
                "scope and crate-state crate slots should match"
            );

            for (next_crate, state) in next_package.iter_mut().zip(package_states) {
                apply_imports(state, env, next_crate).with_context(|| {
                    format!(
                        "while attempting to resolve imports for {}",
                        state.crate_name
                    )
                })?;
            }

            Ok(())
        };

        let dirty_package_count = states.iter_dirty().count();
        if dirty_package_count < PARALLEL_IMPORT_RESOLUTION_PACKAGE_THRESHOLD {
            return next_scopes
                .packages
                .iter_mut()
                .zip(&states.packages)
                .try_for_each(apply_package);
        }

        self.thread_pool()?.install(|| {
            next_scopes
                .packages
                .par_iter_mut()
                .zip(states.packages.par_iter())
                .try_for_each(apply_package)
        })
    }

    fn thread_pool(&mut self) -> anyhow::Result<&rayon::ThreadPool> {
        if self.thread_pool.is_none() {
            let mut builder = rayon::ThreadPoolBuilder::new()
                .thread_name(|index| format!("rg-def-map-imports-{index}"));
            if self.performance_preference == MacroExpansionPerformancePreference::LowerPeakMemory {
                // Import jobs are smaller than macro expansion jobs, but their transient scope
                // tables still multiply with worker count. Honor the same build preference here.
                let worker_count = std::thread::available_parallelism()
                    .map(usize::from)
                    .unwrap_or(LOWER_PEAK_MEMORY_IMPORT_RESOLUTION_THREAD_LIMIT)
                    .min(LOWER_PEAK_MEMORY_IMPORT_RESOLUTION_THREAD_LIMIT);
                builder = builder.num_threads(worker_count);
            }

            self.thread_pool = Some(
                builder
                    .build()
                    .context("while attempting to create import resolution thread pool")?,
            );
        }

        Ok(self
            .thread_pool
            .as_ref()
            .expect("import resolution thread pool should be initialized"))
    }
}

/// Resolution environment used while dirty package scopes are being fixed up.
///
/// Dirty package reads come from fresh crate state and the current fixed-point scope snapshot.
/// Clean package reads fall through to the frozen baseline when one exists.
struct FinalizeResolutionEnv<'a> {
    old: Option<&'a DefMapReadTxn<'a>>,
    states: &'a FinalizeCrateStates,
    current_scopes: &'a ScopeMatrix,
}

impl<'a> FinalizeResolutionEnv<'a> {
    fn new(
        old: Option<&'a DefMapReadTxn<'a>>,
        states: &'a FinalizeCrateStates,
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
            .map(|old| old.def_map(crate_ref))
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
            .as_crate_ref()
            .is_some_and(|crate_ref| self.states.package(crate_ref.package).is_some())
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
            .as_crate_ref()
            .is_some_and(|crate_ref| self.states.package(crate_ref.package).is_some())
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
        self.old
            .map(|old| {
                Ok(old
                    .def_map(crate_ref)?
                    .and_then(|def_map| def_map.local_def(local_def_ref.local_def)))
            })
            .transpose()
            .map(Option::flatten)
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
        self.old
            .map(|old| {
                Ok(old
                    .def_map(crate_ref)?
                    .and_then(|def_map| def_map.local_enum_variant(variant_ref.local_enum_variant)))
            })
            .transpose()
            .map(Option::flatten)
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
            && let Some(old) = self.old
            && let Some(def_map) = old.def_map(crate_ref)?
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
                .map(|old| old.def_map(crate_ref))
                .transpose()?
                .flatten()
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
            .map(|old| old.package(crate_ref.package))
            .transpose()?
            .and_then(|package| {
                package
                    .crate_data(crate_ref.crate_id)
                    .and_then(|data| data.extern_prelude().get(name).copied())
            }))
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
            .map(|old| old.package(crate_ref.package))
            .transpose()?
            .and_then(|package| {
                package
                    .crate_data(crate_ref.crate_id)
                    .and_then(|data| data.prelude())
            }))
    }

    fn root_module(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Option<ModuleRef>, rg_package_store::PackageStoreError> {
        let module = if let Some(state) = self.states.crate_state(crate_ref) {
            Some(state.root_module)
        } else {
            self.old
                .map(|old| old.package(crate_ref.package))
                .transpose()?
                .and_then(|package| {
                    package
                        .crate_data(crate_ref.crate_id)
                        .and_then(|data| data.root_module())
                })
        };

        Ok(module.map(|module| ModuleRef {
            origin: DefMapRef::Crate(crate_ref),
            module,
        }))
    }
}

/// Completes mutable crate states after collection and before freezing.
///
/// Collection records only local facts. This step attaches the edition prelude for each crate,
/// resolves imports and item-position macros against the package graph, and writes the final
/// module scopes back into the collected states.
#[allow(clippy::too_many_arguments)]
pub(super) fn finalize_crate_states(
    old: Option<&DefMapReadTxn<'_>>,
    workspace: &WorkspaceMetadata,
    packages: &[Package],
    item_tree: &ItemTreeDb,
    crate_states: &mut FinalizeCrateStates,
    interners: &mut PackageNameInterners,
    performance_preference: MacroExpansionPerformancePreference,
    generated_module_resolutions: Option<&GeneratedModuleResolutions>,
) -> anyhow::Result<()> {
    // Prelude selection needs the directly declared root modules and crate extern preludes, but it
    // must happen before import resolution because prelude imports participate in normal lookup.
    select_preludes(old, workspace, packages, crate_states, interners)
        .context("while attempting to select crate preludes")?;

    // Once each crate knows its prelude, imports and item-position macros can be resolved through
    // the shared fixed-point loop.
    let mut session = FinalizeScopeSession::new(performance_preference);
    finalize_scopes(
        old,
        item_tree,
        crate_states,
        interners,
        &mut session,
        generated_module_resolutions,
    )
    .context("while attempting to resolve crate scopes")
}

/// Freezes collected crate states into the package payload stored by `DefMapDb`.
pub(super) fn freeze_package(package: &Package, package_states: &[CrateState]) -> DefMapPackage {
    let package_name = package.package_name().to_string();
    let macro_expansion_limits = package_states
        .iter()
        .filter_map(|state| {
            let report = state.macro_expansion_limit.as_ref()?;
            Some(MacroExpansionLimitReport {
                package_name: package_name.clone(),
                crate_name: state.crate_name.clone(),
                groups: report.groups.clone(),
                omitted_call_count: report.omitted_call_count,
            })
        })
        .collect();
    DefMapPackage::new(
        package_name,
        package.edition(),
        package_states.iter().map(freeze_crate_data).collect(),
        macro_expansion_limits,
    )
}

/// Selects the standard prelude module visible from each dirty crate.
///
/// The prelude path depends on the crate_ref edition, and the module it resolves to can live in a
/// clean package. Resolution therefore uses the same dirty-state-plus-old-baseline environment as
/// the later import fixed point.
pub(super) fn select_preludes(
    old: Option<&DefMapReadTxn<'_>>,
    workspace: &WorkspaceMetadata,
    packages: &[Package],
    states: &mut FinalizeCrateStates,
    interners: &mut PackageNameInterners,
) -> anyhow::Result<()> {
    // Prelude lookup only needs directly declared names and crate extern preludes. Using base
    // scopes here keeps the operation independent from later import and macro expansion passes.
    let base_scopes = states.base_scopes();
    let env = FinalizeResolutionEnv::new(old, states, &base_scopes);

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

            // Normal crates use `std` when available. No-std-shaped crates still need the same
            // edition prelude, but rooted at `core`. The core crate itself has a crate-local
            // `prelude` module, so resolve that shape relatively during this early pass.
            // TODO: Parse crate-level `#![no_std]` and use it to select `core` prelude directly
            // and avoid exposing `std` as an automatic extern root for that crate.
            let prelude_paths = [
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
                (workspace_package.name == "core").then(|| {
                    Path::crate_relative_standard_prelude(workspace_package.edition, interner)
                }),
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
/// At the start of a resumed call, resolved generated modules are applied to the scope snapshot
/// retained by the requesting macro batch. The newly collected calls then continue against that
/// same snapshot. An unresolved module records a request and suspends finalization before another
/// global import pass; freezing happens only after every request has been answered and the ordinary
/// fixed point is stable.
pub(super) fn finalize_scopes(
    old: Option<&DefMapReadTxn<'_>>,
    item_tree: &ItemTreeDb,
    states: &mut FinalizeCrateStates,
    interners: &mut PackageNameInterners,
    session: &mut FinalizeScopeSession,
    generated_module_resolutions: Option<&GeneratedModuleResolutions>,
) -> anyhow::Result<()> {
    metric::EXPANSION_PASS_LIMIT.record_count(MAX_MACRO_EXPANSION_PASSES);

    // A resumed build continues the macro pass that emitted the source request. A fresh build has
    // no such pass, so it starts by resolving imports over directly collected declarations.
    let (mut current_scopes, mut resumed_macro_pass) = match session.continuation.take() {
        Some(MacroExpansionContinuation {
            current_scopes,
            next_scan_cursors,
        }) => (current_scopes, Some(next_scan_cursors)),
        None => (states.base_scopes(), None),
    };

    // The surrounding macro was collected before its out-of-line module source became available.
    // Apply the answered module before resuming its macro queue so declarations and calls from the
    // loaded file are visible without an intervening workspace-wide import pass.
    apply_pending_generated_modules(
        item_tree,
        states,
        interners,
        &mut current_scopes,
        generated_module_resolutions,
    )?;

    loop {
        // Source capture resumes inside a macro pass. Every other iteration starts a normal fixed-
        // point round by settling imports over all declarations collected so far.
        let (mut next_scan_cursors, mut needs_import_refresh) = if let Some(scan_cursors) =
            resumed_macro_pass.take()
        {
            // Even a missing answer belongs to an expansion that may have added other imports or
            // declarations. Continue new macro calls first, but require one import refresh before
            // treating the retained scopes as stable.
            (Some(scan_cursors), true)
        } else {
            metric::ROUNDS.inc();
            let timer = metric::TIMING_RESOLVE_IMPORT_SCOPES.start_timer();
            current_scopes =
                resolve_import_scopes(old, states, &mut session.import_executor, current_scopes)?;
            timer.finish();
            (None, false)
        };

        // Macro expansion can introduce more macro calls that are visible in the same scope
        // snapshot. Keep consuming that local queue before paying for another full import pass.
        loop {
            if session.expansion_passes >= MAX_MACRO_EXPANSION_PASSES {
                // Stop expanding but still freeze a coherent def-map. The final import refresh lets
                // names generated before the cap settle into module scopes.
                mark_retryable_macros_skipped_by_limit(states);
                let timer = metric::TIMING_RESOLVE_IMPORT_SCOPES.start_timer();
                current_scopes = resolve_import_scopes(
                    old,
                    states,
                    &mut session.import_executor,
                    current_scopes,
                )?;
                timer.finish();
                freeze_resolved_scopes(old, states, current_scopes)?;
                return Ok(());
            }

            session.expansion_passes += 1;
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
                collect_expansion_attempts(&env, states, scan, &mut session.macro_runtime)?
            };
            timer.finish();

            if expansion_attempts
                .iter()
                .any(|attempt| attempt.needs_expansion())
            {
                // The runtime owns the worker pool and creates it lazily, so projects without
                // expandable declarative macros do not pay its setup cost.
                expand_expansion_attempts(&mut session.macro_runtime, &mut expansion_attempts)?;
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
                    generated_module_resolutions,
                )?
            };
            timer.finish();

            needs_import_refresh |= expansion.changed;
            if expansion.changed {
                current_scopes.censor_proc_macro_exports(states);
            }

            // A generated module request is an external source-loading effect, not a fixed-point
            // boundary. Retain the exact post-batch state and return immediately; after Project
            // supplies the files, calls appended by both the expansion and those files resume from
            // the pre-batch cursor.
            let needs_generated_modules = states.iter_dirty().any(|package_states| {
                package_states
                    .iter()
                    .any(|state| !state.generated_module_requests.is_empty())
            });
            if needs_generated_modules {
                session.continuation = Some(MacroExpansionContinuation {
                    current_scopes,
                    next_scan_cursors: scan_cursors_before_apply,
                });
                return Ok(());
            }

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
            freeze_resolved_scopes(old, states, current_scopes)?;
            return Ok(());
        }
    }
}

fn resolve_import_scopes(
    old: Option<&DefMapReadTxn<'_>>,
    states: &FinalizeCrateStates,
    executor: &mut ImportResolutionExecutor,
    mut current_scopes: ScopeMatrix,
) -> anyhow::Result<ScopeMatrix> {
    metric::IMPORT_RESOLUTION_RUNS.inc();

    loop {
        metric::IMPORT_RESOLUTION_PASSES.inc();
        let mut next_scopes = states.base_scopes();

        // Every iteration starts from the directly declared names, then layers import-derived
        // bindings on top of that snapshot.
        let env = FinalizeResolutionEnv::new(old, states, &current_scopes);
        executor.apply_pass(states, &env, &mut next_scopes)?;
        next_scopes.censor_proc_macro_exports(states);

        if next_scopes == current_scopes {
            return Ok(current_scopes);
        }

        current_scopes = next_scopes;
    }
}

fn freeze_resolved_scopes(
    old: Option<&DefMapReadTxn<'_>>,
    states: &mut FinalizeCrateStates,
    current_scopes: ScopeMatrix,
) -> anyhow::Result<()> {
    // Once the import graph reaches a fixed point, freeze the resolved scopes into the public
    // def-map payload and preserve unresolved imports for query consumers.
    let unresolved_imports = {
        let env = FinalizeResolutionEnv::new(old, states, &current_scopes);
        UnresolvedImports::collect(states, &env)?
    };

    for (package_slot, crate_scopes) in current_scopes.packages.into_iter().enumerate() {
        let Some(crate_scopes) = crate_scopes else {
            continue;
        };
        let package_states = states
            .packages
            .get_mut(package_slot)
            .and_then(Option::as_mut)
            .expect("resolved scopes should exist only for dirty packages");
        assert_eq!(
            package_states.len(),
            crate_scopes.len(),
            "resolved crate scopes should match dirty crate states"
        );

        for (state, scopes) in package_states.iter_mut().zip(crate_scopes) {
            freeze_crate_scopes(state, scopes, &unresolved_imports);
        }
    }

    Ok(())
}

fn freeze_crate_scopes(
    state: &mut CrateState,
    final_scopes: CrateScopeMatrix,
    unresolved_imports: &UnresolvedImports,
) {
    let final_unresolved_imports = unresolved_imports
        .crate_imports(state.crate_ref)
        .expect("unresolved imports should exist for every dirty crate");

    for (module_idx, scope) in final_scopes.into_iter().enumerate() {
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

fn freeze_crate_data(state: &CrateState) -> CrateData {
    // Persist both Cargo-provided roots and explicit crate-root aliases. Queries read this as a
    // prelude rather than pretending any of these names are child modules of the crate root.
    CrateData::new(
        state.cargo_target,
        state.target_kind.clone(),
        state.crate_name.clone(),
        Some(state.root_module),
        state.extern_prelude.freeze(),
        state.prelude,
        state.def_map_builder.clone().build(),
    )
}
