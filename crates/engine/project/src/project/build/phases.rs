//! Builds the retained phase databases for a fresh project snapshot.

use anyhow::Context as _;

use rg_body_ir::{BodyIrBuildPolicy, BodyIrDb};
use rg_def_map::{DefMapDb, DefMapFinalizationStats, PackageSlot};
use rg_item_tree::ItemTreeDb;
use rg_package_store::{PackageEntry, PackageStore};
use rg_parse::ParseDb;
use rg_semantic_ir::SemanticIrDb;
use rg_text::PackageNameInterners;
use rg_workspace::WorkspaceMetadata;

use crate::{
    PackageResidencyPlan,
    cache::{Fingerprint, PackageCacheStore, WorkspaceCachePlan},
    memory::{ProjectMemoryHooks, ProjectMemoryPurgePoint},
    profile::{BuildProfileStage, BuildProfiler, CacheProbeProfile},
    project::{StartupCacheLoad, loading::PackageReadLoaders, package_set::PhasePackageSet},
};

use super::{cache_probe::StartupCacheProbe, stage_memory::StageMemory};

/// Phase payloads built for one project snapshot.
///
/// `ParseDb` keeps package slots, file ids, source paths, and line indexes resident for every
/// package. The heavier retained phases may keep a package resident or leave it offloaded, but
/// DefMap, Semantic IR, and Body IR must agree on that package slot's backing artifact.
pub(super) struct BuiltPhases {
    pub(super) package_source_fingerprints: Vec<Option<Fingerprint>>,
    pub(super) names: PackageNameInterners,
    pub(super) parse: ParseDb,
    pub(super) def_map: DefMapDb,
    pub(super) semantic_ir: SemanticIrDb,
    pub(super) body_ir: BodyIrDb,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build(
    workspace: &WorkspaceMetadata,
    body_ir_policy: BodyIrBuildPolicy,
    package_residency: &PackageResidencyPlan,
    cache_plan: &WorkspaceCachePlan,
    cache_store: &PackageCacheStore,
    startup_cache_load: StartupCacheLoad,
    memory_hooks: &dyn ProjectMemoryHooks,
    finalization_stats: Option<&mut DefMapFinalizationStats>,
    profiler: &mut BuildProfiler,
) -> anyhow::Result<BuiltPhases> {
    let mut stage_memory = StageMemory::default();

    let mut parse = ParseDb::build(workspace).context("while attempting to build parse db")?;
    stage_memory = stage_memory.parse(&parse).checkpoint(
        profiler,
        BuildProfileStage::Parse,
        "after parse",
        &parse,
    );

    let build_plan = PackageBuildPlan::build(
        startup_cache_load,
        body_ir_policy,
        package_residency,
        cache_plan,
        cache_store,
        workspace,
        &mut parse,
    );
    profiler.record_cache_probe(build_plan.cache_probe.clone());
    stage_memory = stage_memory
        .parse(&parse)
        .build_plan(&build_plan.source_packages)
        .checkpoint(
            profiler,
            BuildProfileStage::CacheProbe,
            "after cache probe",
            &build_plan.source_packages,
        );

    let mut names = PackageNameInterners::new(parse.package_count());

    let package_indices = build_plan.source_packages.package_indices();
    let item_tree = ItemTreeDb::build_packages(&mut parse, &package_indices, &mut names)
        .context("while attempting to build item tree db")?;
    stage_memory = stage_memory
        .names(&names)
        .parse(&parse)
        .build_plan(&build_plan.source_packages)
        .item_tree(&item_tree)
        .checkpoint(
            profiler,
            BuildProfileStage::ItemTree,
            "after item-tree",
            &item_tree,
        );

    // Later phases consume file ids, paths, line indexes, and lowered item trees. Body IR reparses
    // syntax file-by-file, so the global parse database can drop full trees before more phase
    // databases start overlapping in memory.
    parse.evict_syntax_trees();
    parse.shrink_to_fit();
    memory_hooks.purge(ProjectMemoryPurgePoint::AfterItemTreeSyntaxEviction);
    stage_memory = stage_memory
        .names(&names)
        .parse(&parse)
        .build_plan(&build_plan.source_packages)
        .item_tree(&item_tree)
        .checkpoint(
            profiler,
            BuildProfileStage::ItemTreeSyntaxEviction,
            "after item-tree syntax eviction",
            &parse,
        );

    let package_source_fingerprints = cache_plan
        .source_fingerprints(workspace.workspace_root(), &parse)
        .context("while attempting to compute package cache source fingerprints")?;
    stage_memory = stage_memory
        .names(&names)
        .parse(&parse)
        .build_plan(&build_plan.source_packages)
        .item_tree(&item_tree)
        .source_fingerprints(&package_source_fingerprints)
        .checkpoint(
            profiler,
            BuildProfileStage::CacheSourceFingerprints,
            "after cache source fingerprints",
            &package_source_fingerprints,
        );

    let loaders = PackageReadLoaders::from_cache(
        cache_plan.clone(),
        cache_store.clone(),
        package_source_fingerprints.clone(),
    );
    let rebuild_subset = build_plan
        .source_packages
        .visible_dependency_subset(workspace);
    // Each retained phase starts as an all-offloaded store. Source-built packages are then
    // replaced in every phase DB; omitted packages remain cache-backed and are loaded lazily
    // through the same package artifact whenever a dependency query needs them.
    let baseline_def_map =
        DefMapDb::from_package_store(offloaded_package_store(parse.package_count()));
    let old_def_map_txn =
        baseline_def_map.read_txn_for_subset(loaders.def_map.clone(), &rebuild_subset);
    let def_map_rebuilder = baseline_def_map.package_rebuilder(
        &old_def_map_txn,
        workspace,
        &parse,
        &item_tree,
        build_plan.source_packages.as_slice(),
        &mut names,
    );
    let def_map = match finalization_stats {
        Some(finalization_stats) => def_map_rebuilder
            .finalization_stats(finalization_stats)
            .build(),
        None => def_map_rebuilder.build(),
    }
    .context("while attempting to build def map db")?;
    drop(old_def_map_txn);
    stage_memory = stage_memory
        .names(&names)
        .parse(&parse)
        .build_plan(&build_plan.source_packages)
        .item_tree(&item_tree)
        .source_fingerprints(&package_source_fingerprints)
        .def_map(&def_map)
        .checkpoint(
            profiler,
            BuildProfileStage::DefMap,
            "after def-map",
            &def_map,
        );

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
    stage_memory = stage_memory
        .names(&names)
        .parse(&parse)
        .build_plan(&build_plan.source_packages)
        .item_tree(&item_tree)
        .source_fingerprints(&package_source_fingerprints)
        .def_map(&def_map)
        .semantic_ir(&semantic_ir)
        .checkpoint(
            profiler,
            BuildProfileStage::SemanticIr,
            "after semantic-ir",
            &semantic_ir,
        );

    // ItemTree is a lowering input, not retained project state. Cache-backed builds only populate
    // packages that missed the artifact cache, but even that sparse tree should disappear before
    // body lowering so retained-memory checkpoints stay focused on durable phase state.
    drop(item_tree);
    stage_memory = stage_memory
        .names(&names)
        .parse(&parse)
        .build_plan(&build_plan.source_packages)
        .source_fingerprints(&package_source_fingerprints)
        .def_map(&def_map)
        .semantic_ir(&semantic_ir)
        .checkpoint_without_retained(
            profiler,
            BuildProfileStage::ItemTreeDrop,
            "after item-tree drop",
        );

    let baseline_body_ir =
        BodyIrDb::from_package_store(offloaded_package_store(parse.package_count()));
    let body_ir = baseline_body_ir
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
        .policy(body_ir_policy)
        .build()
        .context("while attempting to build body ir db")?;
    stage_memory = stage_memory
        .names(&names)
        .parse(&parse)
        .build_plan(&build_plan.source_packages)
        .source_fingerprints(&package_source_fingerprints)
        .def_map(&def_map)
        .semantic_ir(&semantic_ir)
        .body_ir(&body_ir)
        .checkpoint(
            profiler,
            BuildProfileStage::BodyIr,
            "after body-ir",
            &body_ir,
        );
    drop(build_plan);

    parse.evict_syntax_trees();
    parse.shrink_to_fit();
    names.shrink_to_fit();
    stage_memory
        .names(&names)
        .parse(&parse)
        .source_fingerprints(&package_source_fingerprints)
        .def_map(&def_map)
        .semantic_ir(&semantic_ir)
        .body_ir(&body_ir)
        .checkpoint(
            profiler,
            BuildProfileStage::ParseSyntaxEviction,
            "after parse syntax eviction",
            &parse,
        );

    Ok(BuiltPhases {
        package_source_fingerprints,
        names,
        parse,
        def_map,
        semantic_ir,
        body_ir,
    })
}

/// Source-build subset chosen after optional startup cache probing.
///
/// Packages omitted from `source_packages` already have matching offloaded artifacts, so later
/// build phases can read them lazily through package stores instead of lowering them from source.
struct PackageBuildPlan {
    source_packages: PhasePackageSet,
    cache_probe: Option<CacheProbeProfile>,
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
        if !startup_cache_load.is_enabled() {
            return Self {
                source_packages: PhasePackageSet::all(package_count),
                cache_probe: None,
            };
        }

        let mut source_packages = Vec::new();
        let mut cache_probe = StartupCacheProbe::new(
            package_count,
            body_ir_policy,
            package_residency,
            cache_plan,
            cache_store,
            workspace,
            parse,
        );

        for package_idx in 0..package_count {
            let package = PackageSlot(package_idx);
            if cache_probe.should_build_from_source(package) {
                source_packages.push(package);
            }
        }

        Self {
            source_packages: PhasePackageSet::from_packages(source_packages),
            cache_probe: cache_probe.finish(),
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
