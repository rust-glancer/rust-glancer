//! Chooses which module import groups run in each fixed-point wave.
//!
//! A wave starts with a list of active destination modules. Every module in that list reads the
//! same scope snapshot and produces a replacement candidate; none of those candidates is visible
//! to the other jobs in the wave. After all jobs finish, changed scopes are installed together and
//! wake only the import groups which read them.
//!
//! Keeping this synchronous boundary preserves Rust import behavior without rebuilding every
//! module on every pass.

use std::cell::RefCell;

use anyhow::Context as _;
use rayon::prelude::*;
use rg_ir_model::{CrateRef, ImportId, ImportRef, LocalDefRef, LocalEnumVariantRef, ModuleRef};
use rg_macro_runtime::MacroExpansionPerformancePreference;
use rg_std::UniqueVec;
use rg_text::Name;
use rustc_hash::FxHashMap;

use crate::{
    CrateResolutionEnv, ImportKind, LocalDefData, LocalEnumVariantData, LocalEnumVariantEntry,
    ModuleData, ModuleScopeBuilder, ScopeEntryRef, ScopeResolutionEnv, ScopeResolver,
};

use super::{ImportApplicationStats, ModuleSetShape, UnresolvedImports};
use crate::build::finalize::{FinalizeCrateStates, FinalizeResolutionEnv, ScopeMatrix};

// Below this size, dispatching module groups through a worker pool costs more than the import work.
const PARALLEL_IMPORT_RESOLUTION_PACKAGE_THRESHOLD: usize = 32;
const LOWER_PEAK_MEMORY_IMPORT_RESOLUTION_THREAD_LIMIT: usize = 2;

/// Import groups which may still need another run, and the scope reads which can wake them.
///
/// Resolving `use bridge::*` from the crate root reads `bridge`'s module scope. The worklist records
/// that read as “when `bridge` changes, run the root import group again.” These reverse links only
/// grow during one fixed-point run. A path may stop using an older link after another segment
/// changes, but keeping it can only schedule an extra run; it cannot miss a required run.
///
/// Macro expansion starts a new worklist, so links from an older import structure are discarded.
pub(crate) struct ImportWorklist {
    /// Every `use` item owned by each destination module, kept in arena order.
    ///
    /// A module job replays this whole group from its base scope whenever the module becomes
    /// active. The groups do not change during one fixed-point run.
    imports_by_module: FxHashMap<ModuleRef, Vec<ImportId>>,
    /// Reverse edges from a scope read during lookup to the import groups which read it.
    ///
    /// For `use bridge::*` in the crate root, `bridge` points back to the root module. If the
    /// `bridge` scope changes, that edge wakes the root import group for the next wave.
    dependents_by_source: FxHashMap<ModuleRef, UniqueVec<ModuleRef>>,
    /// Destination modules whose import groups will be rebuilt in the next wave.
    ///
    /// The list is sorted by package, crate, and module id before it is evaluated. Besides making
    /// application order stable, that order lets `ModuleSetShape` count adjacent package and crate
    /// runs without allocating sets.
    active_modules: Vec<ModuleRef>,
    /// Latest complete unresolved-import list produced for every dirty module.
    ///
    /// A module's entry is replaced in the same wave as its scope candidate, so the two results
    /// always describe the same import replay when finalization freezes them.
    unresolved_imports: UnresolvedImports,
}

impl ImportWorklist {
    /// Build one import group per destination module and make every group active.
    ///
    /// A module without `use` items cannot gain import-derived bindings, so its base scope needs no
    /// job. Imports stay in arena order inside a group: grouping changes which modules run, not the
    /// order in which one module's imports are replayed.
    pub(crate) fn new(states: &FinalizeCrateStates) -> Self {
        let mut imports_by_module = FxHashMap::<ModuleRef, Vec<ImportId>>::default();
        for package_states in states.iter_dirty() {
            for state in package_states {
                for (import_id, import) in state.def_map_builder.partial().imports_with_ids() {
                    imports_by_module
                        .entry(ModuleRef::krate(state.crate_ref, import.module))
                        .or_default()
                        .push(import_id);
                }
            }
        }

        let mut active_modules = imports_by_module.keys().copied().collect::<Vec<_>>();
        active_modules.sort_unstable_by_key(Self::module_sort_key);

        Self {
            imports_by_module,
            dependents_by_source: FxHashMap::default(),
            active_modules,
            unresolved_imports: UnresolvedImports::empty(states),
        }
    }

    pub(crate) fn active_modules(&self) -> &[ModuleRef] {
        &self.active_modules
    }

    fn imports_for(&self, module: ModuleRef) -> &[ImportId] {
        self.imports_by_module
            .get(&module)
            .map(Vec::as_slice)
            .expect("active module should own an import group")
    }

    /// Install one completed wave and decide which groups run next.
    ///
    /// Each update contains a whole replacement scope, its unresolved imports, and the mutable
    /// scopes it read. The replacement was built from direct declarations plus every import in the
    /// destination module. Replacing the whole result lets bindings disappear or become ambiguous
    /// without separate logic for retracting an earlier import.
    ///
    /// After installing all updates, changed source scopes wake their recorded readers. Those
    /// readers become the active modules for the next wave.
    pub(crate) fn apply_updates(
        &mut self,
        current_scopes: &mut ScopeMatrix,
        updates: Vec<ModuleImportUpdate>,
    ) -> ImportWaveOutcome {
        let evaluated = ModuleSetShape::from_sorted_modules(&self.active_modules);
        let mut stats = ImportApplicationStats::default();
        let mut changed_modules = Vec::new();

        for update in updates {
            stats.merge(update.stats);

            // Only dirty scopes in this matrix can change during the run. A clean package is read
            // from the frozen baseline, so no later wave can make that read stale.
            for source in update.scope_reads {
                if current_scopes.module_scope(source).is_some() {
                    self.dependents_by_source
                        .entry(source)
                        .or_default()
                        .push(update.module);
                }
            }

            self.unresolved_imports
                .replace_module(update.module, update.unresolved_imports);

            if let Some(scope) = update.changed_scope {
                let crate_ref = update
                    .module
                    .origin
                    .as_crate_ref()
                    .expect("crate import should belong to a crate module");
                *current_scopes
                    .module_scope_mut(crate_ref, update.module.module)
                    .expect("active import module should have a mutable scope") = scope;
                changed_modules.push(update.module);
            }
        }

        // Wake readers of every scope changed by this wave. Reads discovered above are already in
        // the reverse map, so a path which reached a deeper module for the first time is included.
        let mut next_active = UniqueVec::new();
        for changed in &changed_modules {
            if let Some(dependents) = self.dependents_by_source.get(changed) {
                next_active.extend(dependents.iter().copied());
            }
        }
        self.active_modules = next_active.into_vec();
        self.active_modules
            .sort_unstable_by_key(Self::module_sort_key);

        // Changed modules are a subset of the sorted update list and keep that order. The same
        // adjacent-run count can therefore describe the evaluated and changed sets.
        let changed = ModuleSetShape::from_sorted_modules(&changed_modules);

        ImportWaveOutcome {
            stats,
            evaluated,
            changed,
            finished: self.active_modules.is_empty(),
        }
    }

    pub(crate) fn into_unresolved_imports(self) -> UnresolvedImports {
        self.unresolved_imports
    }

    fn module_sort_key(module: &ModuleRef) -> (usize, usize, usize) {
        let crate_ref = module
            .origin
            .as_crate_ref()
            .expect("crate import should belong to a crate module");
        (crate_ref.package.0, crate_ref.crate_id.0, module.module.0)
    }
}

/// Results the finalization loop needs after one wave has been installed.
pub(crate) struct ImportWaveOutcome {
    /// Import and glob operations performed by the module jobs.
    pub(crate) stats: ImportApplicationStats,
    /// Packages, crates, and modules which had a job in this wave.
    pub(crate) evaluated: ModuleSetShape,
    /// Packages, crates, and modules whose replacement scope differed from the input snapshot.
    pub(crate) changed: ModuleSetShape,
    /// No import group observes a scope changed by this wave, so another wave cannot change output.
    pub(crate) finished: bool,
}

/// Rebuilt import result for one destination module.
///
/// For a module containing `use crate::types::*`, the job starts with that module's direct
/// declarations and then replays all of its imports. The result also carries every mutable module
/// scope read during lookup and the complete unresolved-import list for this destination.
pub(crate) struct ModuleImportUpdate {
    module: ModuleRef,
    /// Candidate retained only when it differs from this wave's immutable input snapshot.
    ///
    /// Unchanged modules can vastly outnumber changed modules in late waves. Comparing on the
    /// worker which built the candidate lets its namespace allocations die immediately instead of
    /// retaining thousands of equal scopes until the coordinator applies the wave.
    changed_scope: Option<ModuleScopeBuilder>,
    /// Mutable module scopes consulted while resolving this import group.
    scope_reads: Vec<ModuleRef>,
    /// Complete replacement list, not just imports whose status changed in this wave.
    unresolved_imports: Vec<ImportId>,
    stats: ImportApplicationStats,
}

impl ModuleImportUpdate {
    /// Replay one module's imports against this wave's shared scope snapshot.
    fn resolve(
        module: ModuleRef,
        imports: &[ImportId],
        states: &FinalizeCrateStates,
        env: &FinalizeResolutionEnv<'_>,
    ) -> anyhow::Result<Self> {
        let crate_ref = module
            .origin
            .as_crate_ref()
            .expect("crate import should belong to a crate module");
        let state = states
            .crate_state(crate_ref)
            .expect("active import module should belong to a dirty crate");
        // Start from direct declarations. Replaying the whole group on this clean base means an
        // import which stopped resolving naturally disappears from the candidate.
        let mut scope = state
            .base_scopes
            .get(module.module.0)
            .cloned()
            .expect("base scope should exist for every active import module");
        let tracked_env = TrackingResolutionEnv::new(env);
        let resolver = ScopeResolver::new(&tracked_env);
        let mut unresolved_imports = Vec::new();
        let mut stats = ImportApplicationStats::default();

        // Apply imports in source arena order while the tracking environment records module-scope
        // reads. The unresolved list is rebuilt from scratch alongside the candidate scope.
        for import_id in imports {
            let import = state
                .def_map_builder
                .partial()
                .imports()
                .get(import_id.0)
                .expect("worklist import id should remain valid during one fixed-point run");
            assert_eq!(
                import.module, module.module,
                "module import group should contain only its owner's imports"
            );
            stats.imports_evaluated += 1;

            let resolution = resolver
                .apply_import(
                    module,
                    ImportRef {
                        origin: module.origin,
                        import: *import_id,
                    },
                    import,
                    &mut scope,
                )
                .with_context(|| {
                    format!(
                        "while attempting to resolve import {import_id:?} for {}",
                        state.crate_name
                    )
                })?;

            if !resolution.is_resolved() {
                unresolved_imports.push(*import_id);
            }
            if import.kind == ImportKind::Glob {
                stats.glob_imports_evaluated += 1;
                stats.glob_bindings_emitted += resolution.emitted_binding_count();
            }
        }

        // Proc-macro roots expose only macro identities across crate boundaries. Apply that rule
        // before comparison so forbidden value bindings do not look like fixed-point progress.
        state.censor_proc_macro_scope(module.module, &mut scope);
        let changed_scope = (env.current_module_scope(module) != Some(&scope)).then_some(scope);

        Ok(Self {
            module,
            changed_scope,
            scope_reads: tracked_env.into_scope_reads(),
            unresolved_imports,
            stats,
        })
    }
}

/// Runs independent module jobs sequentially or on a reusable worker pool.
///
/// Small waves stay sequential because dispatch costs more than the work. Large waves may run in
/// parallel because every job reads the same snapshot and returns an owned candidate. Each worker
/// temporarily owns one rebuilt scope, so lower-peak-memory mode limits the pool to two workers.
/// The pool lives only as long as the finalization session.
pub(crate) struct ImportResolutionExecutor {
    performance_preference: MacroExpansionPerformancePreference,
    thread_pool: Option<rayon::ThreadPool>,
}

impl ImportResolutionExecutor {
    pub(crate) fn new(performance_preference: MacroExpansionPerformancePreference) -> Self {
        Self {
            performance_preference,
            thread_pool: None,
        }
    }

    /// Resolve every active module without publishing any candidate from this wave.
    ///
    /// The returned vector keeps worklist order even when Rayon runs the jobs in parallel. The
    /// coordinator can therefore install results deterministically after all readers are done.
    pub(crate) fn apply_wave(
        &mut self,
        worklist: &ImportWorklist,
        states: &FinalizeCrateStates,
        env: &FinalizeResolutionEnv<'_>,
    ) -> anyhow::Result<Vec<ModuleImportUpdate>> {
        let resolve_module = |module: &ModuleRef| {
            ModuleImportUpdate::resolve(*module, worklist.imports_for(*module), states, env)
        };

        let active_shape = ModuleSetShape::from_sorted_modules(worklist.active_modules());
        if active_shape.packages < PARALLEL_IMPORT_RESOLUTION_PACKAGE_THRESHOLD {
            return worklist
                .active_modules()
                .iter()
                .map(resolve_module)
                .collect();
        }

        self.thread_pool()
            .context("while attempting to prepare import resolution worker pool")?
            .install(|| {
                worklist
                    .active_modules()
                    .par_iter()
                    .map(resolve_module)
                    .collect()
            })
    }

    fn thread_pool(&mut self) -> anyhow::Result<&rayon::ThreadPool> {
        if self.thread_pool.is_none() {
            let mut builder = rayon::ThreadPoolBuilder::new()
                .thread_name(|index| format!("rg-def-map-imports-{index}"));
            if self.performance_preference == MacroExpansionPerformancePreference::LowerPeakMemory {
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

/// Records mutable module-scope reads made while resolving one import group.
///
/// For `use bridge::User`, looking up `User` records a read of `bridge`. If `bridge` changes after
/// the wave, the destination import group must run again. Structural module data, extern preludes,
/// and local-definition facts do not change during this fixed-point run, so their delegated reads
/// need no tracking.
struct TrackingResolutionEnv<'env, E> {
    inner: &'env E,
    scope_reads: RefCell<UniqueVec<ModuleRef>>,
}

impl<'env, E> TrackingResolutionEnv<'env, E> {
    fn new(inner: &'env E) -> Self {
        Self {
            inner,
            scope_reads: RefCell::new(UniqueVec::new()),
        }
    }

    fn into_scope_reads(self) -> Vec<ModuleRef> {
        self.scope_reads.into_inner().into_vec()
    }
}

impl<E: ScopeResolutionEnv> ScopeResolutionEnv for TrackingResolutionEnv<'_, E> {
    type Error = E::Error;

    fn module_data(&self, module_ref: ModuleRef) -> Result<Option<&ModuleData>, Self::Error> {
        self.inner.module_data(module_ref)
    }

    fn module_scope_entry<'a>(
        &'a self,
        module_ref: ModuleRef,
        name: &str,
    ) -> Result<Option<ScopeEntryRef<'a>>, Self::Error> {
        self.scope_reads.borrow_mut().push(module_ref);
        self.inner.module_scope_entry(module_ref, name)
    }

    fn module_scope_entries<'a>(
        &'a self,
        module_ref: ModuleRef,
    ) -> Result<Vec<(&'a Name, ScopeEntryRef<'a>)>, Self::Error> {
        self.scope_reads.borrow_mut().push(module_ref);
        self.inner.module_scope_entries(module_ref)
    }

    fn local_def_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&LocalDefData>, Self::Error> {
        self.inner.local_def_data(local_def_ref)
    }

    fn local_enum_variant_data(
        &self,
        variant_ref: LocalEnumVariantRef,
    ) -> Result<Option<&LocalEnumVariantData>, Self::Error> {
        self.inner.local_enum_variant_data(variant_ref)
    }

    fn local_enum_variant_entries_for_enum<'a>(
        &'a self,
        enum_def: LocalDefRef,
    ) -> Result<Vec<LocalEnumVariantEntry<'a>>, Self::Error> {
        self.inner.local_enum_variant_entries_for_enum(enum_def)
    }
}

impl<E: CrateResolutionEnv> CrateResolutionEnv for TrackingResolutionEnv<'_, E> {
    fn extern_root(
        &self,
        crate_ref: CrateRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, Self::Error> {
        self.inner.extern_root(crate_ref, name)
    }

    fn prelude_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        self.inner.prelude_module(crate_ref)
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        self.inner.root_module(crate_ref)
    }
}
