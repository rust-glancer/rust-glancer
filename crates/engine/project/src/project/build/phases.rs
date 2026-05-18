//! Builds the retained phase databases for a fresh project snapshot.

use anyhow::Context as _;

use rg_body_ir::{BodyIrBuildPolicy, BodyIrDb};
use rg_def_map::{DefMapDb, PackageSlot};
use rg_item_tree::ItemTreeDb;
use rg_memsize::MemoryRecorder;
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
    project::{StartupCacheLoad, loading::PackageReadLoaders, subset},
};

use super::cache_probe::StartupCacheProbe;

pub(super) struct BuiltPhases {
    pub(super) package_source_fingerprints: Vec<Option<Fingerprint>>,
    pub(super) names: PackageNameInterners,
    pub(super) parse: ParseDb,
    pub(super) def_map: DefMapDb,
    pub(super) semantic_ir: SemanticIrDb,
    pub(super) body_ir: BodyIrDb,
}

/// Snapshot of the phase locals that are still alive at a build checkpoint.
///
/// Each checkpoint fills the objects that exist at that moment; recording skips absent fields so
/// the call sites read as a compact list of live phase state instead of repeating recorder plumbing.
#[derive(Default)]
struct StageMemory<'a> {
    names: Option<&'a PackageNameInterners>,
    parse: Option<&'a ParseDb>,
    build_plan: Option<&'a Vec<PackageSlot>>,
    item_tree: Option<&'a ItemTreeDb>,
    source_fingerprints: Option<&'a Vec<Option<Fingerprint>>>,
    def_map: Option<&'a DefMapDb>,
    semantic_ir: Option<&'a SemanticIrDb>,
    body_ir: Option<&'a BodyIrDb>,
}

impl<'a> StageMemory<'a> {
    fn names(mut self, names: &'a PackageNameInterners) -> Self {
        self.names = Some(names);
        self
    }

    fn parse(mut self, parse: &'a ParseDb) -> Self {
        self.parse = Some(parse);
        self
    }

    fn build_plan(mut self, build_plan: &'a Vec<PackageSlot>) -> Self {
        self.build_plan = Some(build_plan);
        self
    }

    fn item_tree(mut self, item_tree: &'a ItemTreeDb) -> Self {
        self.item_tree = Some(item_tree);
        self
    }

    fn source_fingerprints(mut self, source_fingerprints: &'a Vec<Option<Fingerprint>>) -> Self {
        self.source_fingerprints = Some(source_fingerprints);
        self
    }

    fn def_map(mut self, def_map: &'a DefMapDb) -> Self {
        self.def_map = Some(def_map);
        self
    }

    fn semantic_ir(mut self, semantic_ir: &'a SemanticIrDb) -> Self {
        self.semantic_ir = Some(semantic_ir);
        self
    }

    fn body_ir(mut self, body_ir: &'a BodyIrDb) -> Self {
        self.body_ir = Some(body_ir);
        self
    }

    fn capture(
        self,
        profiler: &mut BuildProfiler,
        stage: BuildProfileStage,
    ) -> StageMemory<'static> {
        profiler.capture_stage_memory(stage, |recorder| self.record(recorder));
        StageMemory::default()
    }

    fn record(&self, recorder: &mut MemoryRecorder) {
        BuildProfiler::record_stage_value(recorder, "names", self.names);
        BuildProfiler::record_stage_value(recorder, "parse", self.parse);
        BuildProfiler::record_stage_value(recorder, "build_plan", self.build_plan);
        BuildProfiler::record_stage_value(recorder, "item_tree", self.item_tree);
        BuildProfiler::record_stage_value(
            recorder,
            "source_fingerprints",
            self.source_fingerprints,
        );
        BuildProfiler::record_stage_value(recorder, "def_map", self.def_map);
        BuildProfiler::record_stage_value(recorder, "semantic_ir", self.semantic_ir);
        BuildProfiler::record_stage_value(recorder, "body_ir", self.body_ir);
    }
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
    profiler: &mut BuildProfiler,
) -> anyhow::Result<BuiltPhases> {
    let mut stage_memory = StageMemory::default();

    let mut parse = ParseDb::build(workspace).context("while attempting to build parse db")?;
    let process_memory = profiler.sample_process_memory();
    let parse_bytes = profiler.measure(&parse);
    profiler.record("after parse", parse_bytes, parse_bytes, process_memory);
    stage_memory = stage_memory
        .parse(&parse)
        .capture(profiler, BuildProfileStage::Parse);

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
    let process_memory = profiler.sample_process_memory();
    let parse_bytes = profiler.measure(&parse);
    let build_plan_bytes = profiler.measure(&build_plan.packages);
    profiler.record(
        "after cache probe",
        build_plan_bytes,
        profiler.sum_retained(&[parse_bytes, build_plan_bytes]),
        process_memory,
    );
    stage_memory = stage_memory
        .parse(&parse)
        .build_plan(&build_plan.packages)
        .capture(profiler, BuildProfileStage::CacheProbe);

    let mut names = PackageNameInterners::new(parse.package_count());

    let package_indices = build_plan.package_indices_to_build();
    let item_tree = ItemTreeDb::build_packages(&mut parse, &package_indices, &mut names)
        .context("while attempting to build item tree db")?;
    let process_memory = profiler.sample_process_memory();
    let names_bytes = profiler.measure(&names);
    let parse_bytes = profiler.measure(&parse);
    let item_tree_bytes = profiler.measure(&item_tree);
    profiler.record(
        "after item-tree",
        item_tree_bytes,
        profiler.sum_retained(&[names_bytes, parse_bytes, build_plan_bytes, item_tree_bytes]),
        process_memory,
    );
    stage_memory = stage_memory
        .names(&names)
        .parse(&parse)
        .build_plan(&build_plan.packages)
        .item_tree(&item_tree)
        .capture(profiler, BuildProfileStage::ItemTree);

    // Later phases consume file ids, paths, line indexes, and lowered item trees. Body IR reparses
    // syntax file-by-file, so the global parse database can drop full trees before more phase
    // databases start overlapping in memory.
    parse.evict_syntax_trees();
    parse.shrink_to_fit();
    memory_hooks.purge(ProjectMemoryPurgePoint::AfterItemTreeSyntaxEviction);
    let process_memory = profiler.sample_process_memory();
    let parse_bytes = profiler.measure(&parse);
    profiler.record(
        "after item-tree syntax eviction",
        parse_bytes,
        profiler.sum_retained(&[names_bytes, parse_bytes, build_plan_bytes, item_tree_bytes]),
        process_memory,
    );
    stage_memory = stage_memory
        .names(&names)
        .parse(&parse)
        .build_plan(&build_plan.packages)
        .item_tree(&item_tree)
        .capture(profiler, BuildProfileStage::ItemTreeSyntaxEviction);

    let package_source_fingerprints = cache_plan
        .source_fingerprints(workspace.workspace_root(), &parse)
        .context("while attempting to compute package cache source fingerprints")?;
    let process_memory = profiler.sample_process_memory();
    let source_fingerprint_bytes = profiler.measure(&package_source_fingerprints);
    profiler.record(
        "after cache source fingerprints",
        source_fingerprint_bytes,
        profiler.sum_retained(&[
            names_bytes,
            parse_bytes,
            build_plan_bytes,
            item_tree_bytes,
            source_fingerprint_bytes,
        ]),
        process_memory,
    );
    stage_memory = stage_memory
        .names(&names)
        .parse(&parse)
        .build_plan(&build_plan.packages)
        .item_tree(&item_tree)
        .source_fingerprints(&package_source_fingerprints)
        .capture(profiler, BuildProfileStage::CacheSourceFingerprints);

    let loaders = PackageReadLoaders::from_cache(
        cache_plan.clone(),
        cache_store.clone(),
        package_source_fingerprints.clone(),
    );
    let rebuild_subset =
        subset::rebuild_packages_with_visible_dependencies(workspace, &build_plan.packages);
    let baseline_def_map =
        DefMapDb::from_package_store(offloaded_package_store(parse.package_count()));
    let old_def_map_txn =
        baseline_def_map.read_txn_for_subset(loaders.def_map.clone(), &rebuild_subset);
    let def_map = baseline_def_map
        .package_rebuilder(
            &old_def_map_txn,
            workspace,
            &parse,
            &item_tree,
            &build_plan.packages,
            &mut names,
        )
        .build()
        .context("while attempting to build def map db")?;
    drop(old_def_map_txn);
    let process_memory = profiler.sample_process_memory();
    let names_bytes = profiler.measure(&names);
    let def_map_bytes = profiler.measure(&def_map);
    profiler.record(
        "after def-map",
        def_map_bytes,
        profiler.sum_retained(&[
            names_bytes,
            parse_bytes,
            build_plan_bytes,
            item_tree_bytes,
            source_fingerprint_bytes,
            def_map_bytes,
        ]),
        process_memory,
    );
    stage_memory = stage_memory
        .names(&names)
        .parse(&parse)
        .build_plan(&build_plan.packages)
        .item_tree(&item_tree)
        .source_fingerprints(&package_source_fingerprints)
        .def_map(&def_map)
        .capture(profiler, BuildProfileStage::DefMap);

    let baseline_semantic_ir =
        SemanticIrDb::from_package_store(offloaded_package_store(parse.package_count()));
    let semantic_ir = baseline_semantic_ir
        .package_rebuilder(
            &item_tree,
            &def_map,
            &build_plan.packages,
            loaders.def_map.clone(),
            loaders.semantic_ir.clone(),
            &rebuild_subset,
        )
        .build()
        .context("while attempting to build semantic ir db")?;
    let process_memory = profiler.sample_process_memory();
    let names_bytes = profiler.measure(&names);
    let semantic_ir_bytes = profiler.measure(&semantic_ir);
    profiler.record(
        "after semantic-ir",
        semantic_ir_bytes,
        profiler.sum_retained(&[
            names_bytes,
            parse_bytes,
            build_plan_bytes,
            item_tree_bytes,
            source_fingerprint_bytes,
            def_map_bytes,
            semantic_ir_bytes,
        ]),
        process_memory,
    );
    stage_memory = stage_memory
        .names(&names)
        .parse(&parse)
        .build_plan(&build_plan.packages)
        .item_tree(&item_tree)
        .source_fingerprints(&package_source_fingerprints)
        .def_map(&def_map)
        .semantic_ir(&semantic_ir)
        .capture(profiler, BuildProfileStage::SemanticIr);

    // ItemTree is a lowering input, not retained project state. Cache-backed builds only populate
    // packages that missed the artifact cache, but even that sparse tree should disappear before
    // body lowering so retained-memory checkpoints stay focused on durable phase state.
    drop(item_tree);
    let process_memory = profiler.sample_process_memory();
    let names_bytes = profiler.measure(&names);
    profiler.record(
        "after item-tree drop",
        None,
        profiler.sum_retained(&[
            names_bytes,
            parse_bytes,
            build_plan_bytes,
            source_fingerprint_bytes,
            def_map_bytes,
            semantic_ir_bytes,
        ]),
        process_memory,
    );
    stage_memory = stage_memory
        .names(&names)
        .parse(&parse)
        .build_plan(&build_plan.packages)
        .source_fingerprints(&package_source_fingerprints)
        .def_map(&def_map)
        .semantic_ir(&semantic_ir)
        .capture(profiler, BuildProfileStage::ItemTreeDrop);

    let baseline_body_ir =
        BodyIrDb::from_package_store(offloaded_package_store(parse.package_count()));
    let body_ir = baseline_body_ir
        .package_rebuilder(
            &parse,
            &def_map,
            &semantic_ir,
            &build_plan.packages,
            &mut names,
            loaders.def_map,
            loaders.semantic_ir,
            &rebuild_subset,
        )
        .policy(body_ir_policy)
        .build()
        .context("while attempting to build body ir db")?;
    let process_memory = profiler.sample_process_memory();
    let names_bytes = profiler.measure(&names);
    let body_ir_bytes = profiler.measure(&body_ir);
    profiler.record(
        "after body-ir",
        body_ir_bytes,
        profiler.sum_retained(&[
            names_bytes,
            parse_bytes,
            build_plan_bytes,
            source_fingerprint_bytes,
            def_map_bytes,
            semantic_ir_bytes,
            body_ir_bytes,
        ]),
        process_memory,
    );
    stage_memory = stage_memory
        .names(&names)
        .parse(&parse)
        .build_plan(&build_plan.packages)
        .source_fingerprints(&package_source_fingerprints)
        .def_map(&def_map)
        .semantic_ir(&semantic_ir)
        .body_ir(&body_ir)
        .capture(profiler, BuildProfileStage::BodyIr);
    drop(build_plan);

    parse.evict_syntax_trees();
    parse.shrink_to_fit();
    let process_memory = profiler.sample_process_memory();
    names.shrink_to_fit();
    let names_bytes = profiler.measure(&names);
    let parse_bytes = profiler.measure(&parse);
    profiler.record(
        "after parse syntax eviction",
        parse_bytes,
        profiler.sum_retained(&[
            names_bytes,
            parse_bytes,
            source_fingerprint_bytes,
            def_map_bytes,
            semantic_ir_bytes,
            body_ir_bytes,
        ]),
        process_memory,
    );
    stage_memory
        .names(&names)
        .parse(&parse)
        .source_fingerprints(&package_source_fingerprints)
        .def_map(&def_map)
        .semantic_ir(&semantic_ir)
        .body_ir(&body_ir)
        .capture(profiler, BuildProfileStage::ParseSyntaxEviction);

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
/// Packages omitted from `packages` already have matching offloaded artifacts, so later build
/// phases can read them lazily through package stores instead of lowering them from source.
struct PackageBuildPlan {
    packages: Vec<PackageSlot>,
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
                packages: (0..package_count).map(PackageSlot).collect(),
                cache_probe: None,
            };
        }

        let mut packages = Vec::new();
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
                packages.push(package);
            }
        }

        Self {
            packages,
            cache_probe: cache_probe.finish(),
        }
    }

    fn package_indices_to_build(&self) -> Vec<usize> {
        self.packages
            .iter()
            .map(|package| package.0)
            .collect::<Vec<_>>()
    }
}

fn offloaded_package_store<T>(package_count: usize) -> PackageStore<T> {
    PackageStore::from_entries(
        (0..package_count)
            .map(|_| PackageEntry::offloaded())
            .collect(),
    )
}
