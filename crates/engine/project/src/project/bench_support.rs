//! Narrow access to production phase construction for the external benchmark target.
//!
//! Cargo compiles a benchmark as a separate crate, so it cannot otherwise call crate-private
//! phase coordination. The narrow entry point below delegates to the same generated-source loop
//! used by fresh and incremental project builds.

use anyhow::Context as _;
use rg_def_map::{DefMapDb, PackageSlot};
use rg_item_tree::ItemTreeDb;
use rg_package_store::{PackageEntry, PackageLoader, PackageStore};
use rg_parse::ParseDb;
use rg_text::PackageNameInterners;
use rg_workspace::WorkspaceMetadata;

use crate::{IndexingPerformancePreference, memory::NoopProjectMemoryHooks};

use super::{generated_modules, package_set::PhasePackageSet};

/// Builds the DefMap phase for an all-source benchmark fixture.
pub fn build_def_map(
    workspace: &WorkspaceMetadata,
    parse: &mut ParseDb,
    item_tree: &mut ItemTreeDb,
    names: &mut PackageNameInterners,
) -> anyhow::Result<DefMapDb> {
    let source_packages =
        PhasePackageSet::from_packages((0..parse.package_count()).map(PackageSlot).collect());
    let baseline = DefMapDb::from_package_store(PackageStore::from_entries(
        (0..parse.package_count())
            .map(|_| PackageEntry::offloaded())
            .collect(),
    ));
    let visible_packages = source_packages.visible_dependency_subset(workspace);
    let baseline_read = baseline.read_txn_for_subset(
        PackageLoader::resident_only("all-source DefMap benchmark"),
        &visible_packages,
    );

    generated_modules::build_packages(
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
