//! Narrow access to production phase construction for the external benchmark target.
//!
//! Cargo compiles a benchmark as a separate crate, so it cannot otherwise call crate-private
//! phase coordination. These entry points prepare all-source baselines, then delegate to the same
//! selected-package construction paths used by fresh and incremental project construction.

use anyhow::Context as _;
use rg_body_ir::{BodyIrBuildPolicy, BodyIrDb, PackageBodiesCoverage};
use rg_def_map::{DefMapDb, PackageSlot};
use rg_item_tree::ItemTreeDb;
use rg_package_store::{PackageEntry, PackageLoader, PackageStore, PackageSubset};
use rg_parse::ParseDb;
use rg_semantic_ir::SemanticIrDb;
use rg_text::PackageNameInterners;
use rg_workspace::WorkspaceMetadata;

use crate::{IndexingPerformancePreference, memory::NoopProjectMemoryHooks};

use super::{macro_source_files, package_set::PhasePackageSet};

/// Builds ItemTree for every parsed benchmark package through the selected-package lowerer.
pub fn build_item_tree(
    parse: &mut ParseDb,
    names: &mut PackageNameInterners,
) -> anyhow::Result<ItemTreeDb> {
    let packages = (0..parse.package_count()).collect::<Vec<_>>();
    ItemTreeDb::build_packages(parse, &packages, names)
        .context("while attempting to build benchmark ItemTree packages")
}

/// Builds the DefMap phase for an all-source benchmark fixture.
pub fn build_def_map(
    workspace: &WorkspaceMetadata,
    parse: &mut ParseDb,
    item_tree: &mut ItemTreeDb,
    names: &mut PackageNameInterners,
) -> anyhow::Result<DefMapDb> {
    let source_packages =
        PhasePackageSet::from_packages((0..parse.package_count()).map(PackageSlot).collect());
    let baseline = DefMapDb::from_package_store(PackageStore::all_offloaded(parse.package_count()));
    let visible_packages = source_packages.visible_dependency_subset(workspace);
    let baseline_read = baseline.read_txn_for_subset(
        PackageLoader::resident_only("all-source DefMap benchmark"),
        &visible_packages,
    );

    macro_source_files::build_packages(
        &baseline,
        &baseline_read,
        workspace,
        parse,
        item_tree,
        &source_packages,
        names,
        IndexingPerformancePreference::default().macro_expansion_preference(),
        &NoopProjectMemoryHooks,
    )
    .context("while attempting to build benchmark DefMap through project coordination")
}

/// Builds Semantic IR for every benchmark package through the baseline-replacement path.
pub fn build_semantic_ir(
    item_tree: &ItemTreeDb,
    def_map: &DefMapDb,
) -> anyhow::Result<SemanticIrDb> {
    let package_count = def_map.package_count();
    let packages = (0..package_count).map(PackageSlot).collect::<Vec<_>>();
    let subset = PackageSubset::all(package_count);
    let baseline = SemanticIrDb::from_package_store(PackageStore::all_offloaded(package_count));

    baseline
        .build_packages(
            item_tree,
            def_map,
            &packages,
            PackageLoader::resident_only("all-source benchmark DefMap"),
            PackageLoader::resident_only("all-source benchmark Semantic IR"),
            &subset,
        )
        .context("while attempting to build benchmark Semantic IR packages")
}

/// Builds Body IR for every benchmark package through the selected-package builder.
pub fn build_body_ir(
    parse: &ParseDb,
    def_map: &DefMapDb,
    semantic_ir: &SemanticIrDb,
    names: &mut PackageNameInterners,
) -> anyhow::Result<BodyIrDb> {
    let package_count = parse.package_count();
    let packages = (0..package_count).map(PackageSlot).collect::<Vec<_>>();
    let subset = PackageSubset::all(package_count);
    // Every package is selected, so this provisional coverage is replaced before it can be read.
    let baseline = BodyIrDb::from_package_store(PackageStore::from_entries(
        (0..package_count)
            .map(|_| PackageEntry::offloaded_with(PackageBodiesCoverage::from_crates(Vec::new())))
            .collect(),
    ));
    let indexing_preference = IndexingPerformancePreference::default();

    baseline
        .builder(
            parse,
            def_map,
            semantic_ir,
            &packages,
            names,
            PackageLoader::resident_only("all-source benchmark DefMap"),
            PackageLoader::resident_only("all-source benchmark Semantic IR"),
            &subset,
        )
        .configured_bodies(BodyIrBuildPolicy::default())
        .worker_limit(indexing_preference.body_ir_worker_limit())
        .build()
        .context("while attempting to build benchmark Body IR packages")
}
