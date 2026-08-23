//! Builds the retained phase databases for a fresh project snapshot.
//!
//! Source-built package inventories remain open through DefMap construction because supported
//! macro expansion can reveal more module files. Only after that discovery loop settles are the
//! source set and the fingerprints retained by the project finalized.

use anyhow::Context as _;

use rg_body_ir::{BodyIrBuildPolicy, BodyIrDb, CrateBodiesCoverage, PackageBodiesCoverage};
use rg_def_map::{DefMapDb, PackageSlot};
use rg_item_tree::ItemTreeDb;
use rg_package_store::{PackageEntry, PackageStore};
use rg_parse::ParseDb;
use rg_semantic_ir::SemanticIrDb;
use rg_std::{MemorySize, Shrink};
use rg_text::PackageNameInterners;
use rg_workspace::WorkspaceMetadata;

use crate::{
    IndexingPerformancePreference, PackageResidencyPlan,
    cache::{Fingerprint, PackageCacheStore, WorkspaceCachePlan},
    memory::{ProjectMemoryHooks, ProjectMemoryPurgePoint},
    profile::{BuildMemorySampler, metric},
    project::{
        SplitIndexingMode, StartupCacheLoad, generated_modules, loading::PackageReadLoaders,
        package_set::PhasePackageSet, stats::MacroExpansionLimitBuildSummary,
    },
};

use super::{
    cache_probe::{StartupCacheProbe, StartupPackageSelection},
    checkpoint_memory::CheckpointMemory,
};

macro_rules! checkpoint_memory {
    ($($value:expr),+ $(,)?) => {{
        let mut memory = CheckpointMemory::default();
        $(
            memory = memory.merge(CheckpointMemory::from(&$value));
        )+
        memory
    }};
}

/// Phase payloads built for one project snapshot.
///
/// `ParseDb` keeps package slots, file ids, source paths, and line indexes resident for every
/// package. The heavier retained phases may keep a package resident or leave it offloaded, but
/// DefMap, Semantic IR, and Body IR must agree on that package slot's backing artifact.
pub(super) struct BuiltPhases {
    pub(super) package_source_fingerprints: Vec<Option<Fingerprint>>,
    pub(super) names: PackageNameInterners,
    pub(super) parse: ParseDb,
    pub(super) macro_expansion_limit_summary: MacroExpansionLimitBuildSummary,
    pub(super) def_map: DefMapDb,
    pub(super) semantic_ir: SemanticIrDb,
    pub(super) body_ir: BodyIrDb,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build(
    workspace: &WorkspaceMetadata,
    body_ir_policy: BodyIrBuildPolicy,
    indexing_preference: IndexingPerformancePreference,
    package_residency: &PackageResidencyPlan,
    cache_plan: &WorkspaceCachePlan,
    cache_store: &PackageCacheStore,
    startup_cache_load: StartupCacheLoad,
    split_indexing_mode: SplitIndexingMode,
    memory_hooks: &dyn ProjectMemoryHooks,
    sampler: &mut BuildMemorySampler,
) -> anyhow::Result<BuiltPhases> {
    // ---------------------
    // 1. Parse all packages
    // ---------------------
    let mut parse = ParseDb::build(workspace).context("while attempting to build parse db")?;
    let memory = checkpoint_memory!(parse);
    memory.checkpoint(sampler, metric::PARSE_MEMORY, &parse);

    // -------------------------------
    // 2. Choose source-built packages
    // -------------------------------
    let mut build_plan = PackageBuildPlan::build(
        startup_cache_load,
        body_ir_policy,
        package_residency,
        cache_plan,
        cache_store,
        workspace,
        &mut parse,
    );
    let memory = checkpoint_memory!(parse, build_plan);
    memory.checkpoint(sampler, metric::CACHE_PROBE_MEMORY, &build_plan);

    let mut names = PackageNameInterners::new(parse.package_count());

    // -------------------
    // 3. Lower initial item trees
    // -------------------
    let package_indices = build_plan.source_packages.package_indices();
    let mut item_tree = ItemTreeDb::build_packages(&mut parse, &package_indices, &mut names)
        .context("while attempting to build item tree db")?;
    let memory = checkpoint_memory!(names, parse, build_plan, item_tree);
    memory.checkpoint(sampler, metric::ITEM_TREE_MEMORY, &item_tree);

    // -------------------------
    // 4. Evict initial item-tree syntax
    // -------------------------
    // Later phases consume file ids, paths, line indexes, and lowered item trees. Body IR reparses
    // syntax file-by-file, so the global parse database can drop full trees before more phase
    // databases start overlapping in memory.
    parse.evict_syntax_trees();
    parse.shrink_to_fit();
    memory_hooks.purge(ProjectMemoryPurgePoint::AfterItemTreeSyntaxEviction);
    let memory = checkpoint_memory!(names, parse, build_plan, item_tree);
    memory.checkpoint(sampler, metric::ITEM_TREE_SYNTAX_EVICTION_MEMORY, &parse);

    // -------------------------------
    // 5. Prepare cache-backed loaders
    // -------------------------------
    // Cache-hit package tables are already final and these fingerprints validate their artifacts.
    // Source-built package tables can still grow during DefMap, so exclude those slots from all
    // cache readers and treat their values as provisional until discovery settles.
    let mut source_fingerprints = cache_plan
        .source_fingerprints(workspace.workspace_root(), &parse)
        .context("while attempting to compute package cache source fingerprints")?;
    let memory = checkpoint_memory!(names, parse, build_plan, item_tree, source_fingerprints);
    memory.checkpoint(
        sampler,
        metric::CACHE_SOURCE_FINGERPRINTS_MEMORY,
        &source_fingerprints,
    );

    let loaders = PackageReadLoaders::from_cache_excluding(
        cache_plan.clone(),
        cache_store.clone(),
        source_fingerprints.clone(),
        build_plan.source_packages.as_slice(),
    );
    let rebuild_subset = build_plan
        .source_packages
        .visible_dependency_subset(workspace);

    // ----------------
    // 6. Build def-map
    // ----------------
    // Each retained phase starts as an all-offloaded store. Source-built packages are then
    // replaced in every phase DB; omitted packages remain cache-backed and are loaded lazily
    // through the same package artifact whenever a dependency query needs them.
    let baseline_def_map =
        DefMapDb::from_package_store(offloaded_package_store(parse.package_count()));
    let old_def_map_txn =
        baseline_def_map.read_txn_for_subset(loaders.def_map.clone(), &rebuild_subset);
    // Macro expansion may now request an out-of-line module. Keep Parse and ItemTree mutable while
    // the coordinator answers complete batches and resumes the same DefMap construction state.
    let def_map = generated_modules::rebuild_packages(
        &baseline_def_map,
        &old_def_map_txn,
        workspace,
        &mut parse,
        &mut item_tree,
        &build_plan.source_packages,
        &mut names,
        indexing_preference.macro_expansion_preference(),
        memory_hooks,
    )
    .context("while attempting to build def map db")?;
    drop(old_def_map_txn);

    // The fixed point has finalized every source-built package file table. Seal that exact source
    // union now, then replace only its provisional fingerprints. Cache-backed loaders keep the
    // validated values captured above, and dirty packages never read an old payload through them.
    parse.seal_sources();
    let provisional_source_fingerprints = build_plan
        .source_packages
        .as_slice()
        .iter()
        .map(|package| {
            (
                *package,
                source_fingerprints.get(package.0).copied().flatten(),
            )
        })
        .collect::<Vec<_>>();
    cache_plan
        .refresh_source_fingerprints(
            workspace.workspace_root(),
            &parse,
            &mut source_fingerprints,
            build_plan.source_packages.as_slice(),
        )
        .context("while attempting to finalize source-built package fingerprints")?;
    let changed_fingerprint_count = provisional_source_fingerprints
        .into_iter()
        .filter(|(package, provisional)| {
            source_fingerprints.get(package.0).copied().flatten() != *provisional
        })
        .count();
    metric::GENERATED_MODULE_CACHE_FINGERPRINT_CHANGES
        .add(changed_fingerprint_count.try_into().unwrap_or(u64::MAX));
    memory_hooks.purge(ProjectMemoryPurgePoint::AfterDefMapBuild);
    let memory = checkpoint_memory!(
        names,
        parse,
        build_plan,
        item_tree,
        source_fingerprints,
        def_map,
    );
    memory.checkpoint(sampler, metric::DEF_MAP_MEMORY, &def_map);

    // --------------------
    // 7. Build semantic IR
    // --------------------
    let baseline_semantic_ir =
        SemanticIrDb::from_package_store(offloaded_package_store(parse.package_count()));
    let semantic_ir = baseline_semantic_ir
        .package_rebuilder(
            &item_tree,
            &def_map,
            build_plan.source_packages.as_slice(),
            loaders.def_map.clone(),
            loaders.semantic_ir.clone(),
            &rebuild_subset,
        )
        .build()
        .context("while attempting to build semantic ir db")?;
    let memory = checkpoint_memory!(
        names,
        parse,
        build_plan,
        item_tree,
        source_fingerprints,
        def_map,
        semantic_ir,
    );
    memory.checkpoint(sampler, metric::SEMANTIC_IR_MEMORY, &semantic_ir);

    // ----------------------------
    // 8. Drop transient item trees
    // ----------------------------
    // ItemTree is a lowering input, not retained project state. Cache-backed builds only populate
    // packages that missed the artifact cache, but even that sparse tree should disappear before
    // body lowering so retained-memory checkpoints stay focused on durable phase state.
    drop(item_tree);
    let memory = checkpoint_memory!(
        names,
        parse,
        build_plan,
        source_fingerprints,
        def_map,
        semantic_ir,
    );
    memory.checkpoint_without_retained(sampler, metric::ITEM_TREE_DROP_MEMORY);

    // ----------------
    // 9. Build body IR
    // ----------------
    // Cache-hit slots retain only the coverage already accepted by startup probing. Source-built
    // slots use their conservative seed until the rebuilder replaces them below.
    let body_ir_entries = std::mem::take(&mut build_plan.body_ir_coverage)
        .into_iter()
        .map(PackageEntry::offloaded_with)
        .collect();
    let baseline_body_ir =
        BodyIrDb::from_package_store(PackageStore::from_entries(body_ir_entries));
    let mut body_rebuilder = baseline_body_ir
        .package_rebuilder(
            &parse,
            &def_map,
            &semantic_ir,
            build_plan.source_packages.as_slice(),
            &mut names,
            loaders.def_map,
            loaders.semantic_ir,
            &rebuild_subset,
        )
        .worker_limit(indexing_preference.body_ir_worker_limit());
    body_rebuilder = match split_indexing_mode {
        SplitIndexingMode::Full => body_rebuilder.configured_bodies(body_ir_policy),
        SplitIndexingMode::EarlyStart => body_rebuilder.coverage_only(body_ir_policy),
    };
    let body_ir = body_rebuilder
        .build()
        .context("while attempting to build body ir db")?;
    // This validation covers both captured late files and the missing-path probes that answered
    // generated module requests, so a concurrent source change rejects the whole candidate.
    parse
        .validate_saved_sources()
        .context("while attempting to validate captured project source generation")?;
    memory_hooks.purge(ProjectMemoryPurgePoint::AfterBodyIrBuild);
    let memory = checkpoint_memory!(
        names,
        parse,
        build_plan,
        source_fingerprints,
        def_map,
        semantic_ir,
        body_ir,
    );
    memory.checkpoint(sampler, metric::BODY_IR_MEMORY, &body_ir);

    // Capture the diagnostic before residency can offload a freshly built package. Only package
    // slots expanded from source belong to this build; cache hits carry older artifacts and did
    // not encounter the expansion guard during this indexing operation.
    let macro_expansion_limit_summary =
        MacroExpansionLimitBuildSummary::capture(&def_map, build_plan.source_packages.as_slice());
    drop(build_plan);

    // --------------------------
    // 10. Compact retained state
    // --------------------------
    parse.evict_syntax_trees();
    parse.evict_saved_source_text();
    parse.shrink_to_fit();
    Shrink::shrink_to_fit(&mut names);
    let memory = checkpoint_memory!(
        names,
        parse,
        source_fingerprints,
        def_map,
        semantic_ir,
        body_ir,
    );
    memory.checkpoint(sampler, metric::PARSE_SYNTAX_EVICTION_MEMORY, &parse);

    Ok(BuiltPhases {
        package_source_fingerprints: source_fingerprints,
        names,
        parse,
        macro_expansion_limit_summary,
        def_map,
        semantic_ir,
        body_ir,
    })
}

/// Phase inputs retained after optional startup cache probing.
///
/// Packages omitted from `source_packages` already have matching offloaded artifacts, so later
/// build phases can read them lazily instead of lowering them from source. Body IR coverage gives
/// each provisional package-store entry the small state it needs before the payload is available.
#[derive(MemorySize)]
pub(super) struct PackageBuildPlan {
    source_packages: PhasePackageSet,
    /// Exact cache-hit coverage plus a conservative seed for packages rebuilt immediately.
    body_ir_coverage: Vec<PackageBodiesCoverage>,
}

impl PackageBuildPlan {
    /// Decides which packages still need source lowering for this build.
    ///
    /// For cache hits we also restore the parse snapshot from the artifact. That keeps source file
    /// ids, paths, and line indexes in sync with the offloaded phase payloads that lazy readers will
    /// load later.
    fn build(
        startup_cache_load: StartupCacheLoad,
        body_ir_policy: BodyIrBuildPolicy,
        package_residency: &PackageResidencyPlan,
        cache_plan: &WorkspaceCachePlan,
        cache_store: &PackageCacheStore,
        workspace: &WorkspaceMetadata,
        parse: &mut ParseDb,
    ) -> Self {
        let package_count = parse.package_count();
        let package_selections = if startup_cache_load.is_enabled() {
            let mut cache_probe = StartupCacheProbe::new(
                package_count,
                body_ir_policy,
                package_residency,
                cache_plan,
                cache_store,
                workspace,
                parse,
            );
            cache_probe.select()
        } else {
            (0..package_count)
                .map(|_| StartupPackageSelection::BuildFromSource)
                .collect()
        };
        assert_eq!(
            package_selections.len(),
            package_count,
            "startup selection should cover every package slot",
        );
        let source_packages = PhasePackageSet::from_packages(
            package_selections
                .iter()
                .enumerate()
                .filter_map(|(package_idx, selection)| {
                    matches!(selection, StartupPackageSelection::BuildFromSource)
                        .then_some(PackageSlot(package_idx))
                })
                .collect(),
        );

        // BodyIrDb needs one package-shaped offloaded entry before its rebuilder starts. Cache hits
        // already have exact coverage. Source packages receive a conservative policy-shaped value
        // that is replaced by their actual build output before the database escapes.
        let body_ir_coverage = parse
            .packages()
            .iter()
            .zip(package_selections)
            .map(|(package, selection)| match selection {
                StartupPackageSelection::Cached { body_ir_coverage } => body_ir_coverage,
                StartupPackageSelection::BuildFromSource => PackageBodiesCoverage::from_crates(
                    package
                        .targets()
                        .iter()
                        .map(|target| {
                            if body_ir_policy.should_lower_target(package, target) {
                                CrateBodiesCoverage::Missing
                            } else {
                                CrateBodiesCoverage::SkippedByPolicy
                            }
                        })
                        .collect(),
                ),
            })
            .collect();

        Self {
            source_packages,
            body_ir_coverage,
        }
    }
}

fn offloaded_package_store<T>(package_count: usize) -> PackageStore<T> {
    PackageStore::from_entries(
        (0..package_count)
            .map(|_| PackageEntry::offloaded())
            .collect(),
    )
}
