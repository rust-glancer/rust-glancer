//! Rebuilds selected packages inside an existing project snapshot.

use std::sync::Arc;

use anyhow::Context as _;

use rg_body_ir::BodyIrFile;
use rg_def_map::PackageSlot;
use rg_item_tree::ItemTreeDb;
use rg_std::Shrink;

use crate::{
    ProjectMemoryPurgePoint,
    profile::BuildMemorySampler,
    project::{
        SplitIndexingMode, StartupCacheLoad, build, loading::PackageReadLoaders,
        offloading::ResidencyApplication, package_set::PhasePackageSet, state::ProjectState,
    },
};

pub(super) fn rebuild_packages(
    state: &mut ProjectState,
    packages: &[PackageSlot],
) -> anyhow::Result<()> {
    if packages.is_empty() {
        return Ok(());
    }

    let plan = PackageRebuildPlan::saved(packages, state.split_indexing_mode);
    match try_rebuild_packages(state, plan) {
        Ok(()) => {
            state
                .memory_hooks
                .purge(ProjectMemoryPurgePoint::AfterPackageRebuild);
            Ok(())
        }
        Err(error) if ProjectState::is_recoverable_cache_load_failure(&error) => {
            ResidencyApplication::failure_recovery(state).with_context(|| {
                format!(
                    "while attempting to recover analysis project after package cache load failed during package rebuild: {error}",
                )
            })
        }
        Err(error) => Err(error),
    }
}

/// Rebuild dirty packages without running saved-project residency or allocator cleanup.
///
/// The rebuilt payload stays available through the matching query. Its caller owns the later
/// request-memory release and allocator purge; doing either here would put cleanup inside the
/// interactive overlay build.
pub(super) fn rebuild_dirty_overlay_packages(
    state: &mut ProjectState,
    packages: &[PackageSlot],
    body_files: &[BodyIrFile],
    can_reuse_saved_item_lookup_indexes: bool,
) -> anyhow::Result<()> {
    if packages.is_empty() {
        return Ok(());
    }

    try_rebuild_packages(
        state,
        PackageRebuildPlan::dirty_overlay(
            packages,
            body_files,
            can_reuse_saved_item_lookup_indexes,
        ),
    )
}

fn try_rebuild_packages(
    state: &mut ProjectState,
    plan: PackageRebuildPlan<'_>,
) -> anyhow::Result<()> {
    // Rebuilding one package can resolve names through its dependencies, but unrelated packages
    // should stay offloaded so save handling does not recreate full-project spikes.
    let rebuild_subset = plan
        .source_packages
        .visible_dependency_subset(&state.workspace);
    let package_indices = plan.source_packages.package_indices();

    // Saved rebuilding replaces the package file table before discovering modules again. Keeping
    // the old table would make removed modules permanent members of source validation and cache
    // snapshots even after the new ItemTree stopped reaching them. Dirty overlays are ephemeral
    // and deliberately preserve the published file ids instead.
    if matches!(plan.residency, RebuildResidency::RestoreSavedState) {
        state
            .parse
            .reset_packages_from_workspace(&state.workspace, &package_indices)
            .context("while attempting to reset rebuilt package source roots")?;
    }

    let loaders = PackageReadLoaders::new(state);
    let old_def_map_txn = state
        .def_map
        .read_txn_for_subset(loaders.def_map.clone(), &rebuild_subset);

    let item_tree =
        ItemTreeDb::build_packages(&mut state.parse, &package_indices, &mut state.names)
            .context("while attempting to rebuild affected item-tree packages")?;
    state.parse.seal_sources();

    // Rebuilds follow the same lifetime rule as fresh indexing: item-tree owns the lowered
    // declarations, and body lowering reparses only the files it needs.
    state.parse.evict_syntax_trees();
    state.parse.shrink_to_fit();
    state
        .memory_hooks
        .purge(ProjectMemoryPurgePoint::AfterItemTreeSyntaxEviction);

    // Fresh indexing exposes more allocator purge boundaries because it can build the whole
    // workspace at once. Package rebuilds are usually smaller and can run on save or dirty-overlay
    // paths, so we avoid adding extra def-map/body purges to the interactive rebuild path.
    let def_map = state
        .def_map
        .package_rebuilder(
            &old_def_map_txn,
            &state.workspace,
            &state.parse,
            &item_tree,
            plan.source_packages.as_slice(),
            &mut state.names,
        )
        .performance_preference(state.indexing_preference.macro_expansion_preference())
        .build()
        .context("while attempting to rebuild affected def-map packages")?;
    drop(old_def_map_txn);
    let semantic_ir = state
        .semantic_ir
        .package_rebuilder(
            &item_tree,
            &def_map,
            plan.source_packages.as_slice(),
            loaders.def_map.clone(),
            loaders.semantic_ir.clone(),
            &rebuild_subset,
        )
        .build()
        .context("while attempting to rebuild affected semantic IR packages")?;

    // Dirty body edits often leave crate declarations unchanged. Compare the rebuilt DefMap and
    // Semantic IR with the compact saved fingerprints now, while those new packages are resident.
    // Other rebuild modes keep the ordinary fresh-index construction path.
    let item_lookup_indexes_unchanged = if plan.can_reuse_saved_item_lookup_indexes {
        loaders
            .item_lookup_indexes_unchanged(&def_map, &semantic_ir, plan.source_packages.as_slice())
            .context("while attempting to compare dirty item lookup indexes with saved analysis")?
    } else {
        false
    };
    let mut body_rebuilder = state.body_ir.package_rebuilder(
        &state.parse,
        &def_map,
        &semantic_ir,
        plan.body_packages.as_slice(),
        &mut state.names,
        loaders.def_map.clone(),
        loaders.semantic_ir.clone(),
        &rebuild_subset,
    );
    if item_lookup_indexes_unchanged {
        // The compact cache-probe fingerprints cover every replaced package's lookup facts and
        // visibility edges. The Body IR loader belongs to the same loader set that read those
        // probes, so it can now load the saved indexes without materializing the old Semantic IR
        // or walking the dependency closure a second time.
        body_rebuilder = body_rebuilder.reuse_item_lookup_indexes(loaders.body_ir.clone());
    }
    body_rebuilder = match plan.body_scope {
        BodyRebuildScope::ConfiguredBodies => {
            body_rebuilder.configured_bodies(state.body_ir_policy)
        }
        BodyRebuildScope::CoverageOnly => body_rebuilder.coverage_only(state.body_ir_policy),
        BodyRebuildScope::DirtyFiles(files) => body_rebuilder.selected_files(files.to_vec()),
    };
    let (body_ir, trait_selection_sessions) = match plan.body_scope {
        // Dirty queries can immediately reuse the crate-semantic solver program built while these
        // same bodies were resolved. Saved rebuilds have no request boundary to own that state and
        // deliberately keep the ordinary drop-on-build behavior.
        BodyRebuildScope::DirtyFiles(_) => body_rebuilder.build_with_trait_selection_sessions(),
        BodyRebuildScope::ConfiguredBodies | BodyRebuildScope::CoverageOnly => {
            body_rebuilder.build().map(|body_ir| (body_ir, Vec::new()))
        }
    }
    .context("while attempting to rebuild affected body IR packages")?;
    match plan.residency {
        RebuildResidency::RestoreSavedState => state
            .parse
            .validate_saved_sources()
            .context("while attempting to validate captured project source generation")?,
        // Only these packages received newly derived analysis. Dependencies still come from the
        // already-validated saved generation, while every source read made during this rebuild
        // independently verifies the frozen descriptor before returning text.
        RebuildResidency::KeepResident => state
            .parse
            .validate_saved_sources_in_packages(&package_indices)
            .context("while attempting to validate dirty overlay source packages")?,
    }
    state.parse.evict_saved_source_text();

    // ItemTree is a transient rebuild input. Drop it before pruning the weak interner so names
    // that did not survive into retained DBs are no longer treated as live.
    drop(item_tree);

    state.def_map = def_map;
    state.semantic_ir = semantic_ir;
    state.body_ir = body_ir;
    if matches!(plan.body_scope, BodyRebuildScope::DirtyFiles(_)) {
        state.install_query_cache(loaders.clone(), trait_selection_sessions);
    } else {
        state.clear_query_cache();
    }
    Shrink::shrink_to_fit(&mut state.names);
    if matches!(plan.residency, RebuildResidency::RestoreSavedState) {
        ResidencyApplication::restore(state, plan.source_packages.as_slice())
            .apply()
            .context("while attempting to apply package cache residency after package rebuild")?;
    }

    Ok(())
}

struct PackageRebuildPlan<'a> {
    source_packages: PhasePackageSet,
    body_packages: PhasePackageSet,
    body_scope: BodyRebuildScope<'a>,
    residency: RebuildResidency,
    can_reuse_saved_item_lookup_indexes: bool,
}

impl<'a> PackageRebuildPlan<'a> {
    fn saved(packages: &'a [PackageSlot], split_indexing_mode: SplitIndexingMode) -> Self {
        let body_scope = match split_indexing_mode {
            SplitIndexingMode::Full => BodyRebuildScope::ConfiguredBodies,
            SplitIndexingMode::EarlyStart => BodyRebuildScope::CoverageOnly,
        };
        Self {
            source_packages: PhasePackageSet::from_slice(packages),
            body_packages: PhasePackageSet::from_slice(packages),
            body_scope,
            residency: RebuildResidency::RestoreSavedState,
            can_reuse_saved_item_lookup_indexes: false,
        }
    }

    fn dirty_overlay(
        source_packages: &'a [PackageSlot],
        body_files: &'a [BodyIrFile],
        can_reuse_saved_item_lookup_indexes: bool,
    ) -> Self {
        Self {
            source_packages: PhasePackageSet::from_slice(source_packages),
            body_packages: PhasePackageSet::from_body_files(body_files),
            body_scope: BodyRebuildScope::DirtyFiles(body_files),
            residency: RebuildResidency::KeepResident,
            can_reuse_saved_item_lookup_indexes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyRebuildScope<'a> {
    ConfiguredBodies,
    CoverageOnly,
    DirtyFiles(&'a [BodyIrFile]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RebuildResidency {
    RestoreSavedState,
    KeepResident,
}

pub(crate) fn rebuild_resident_from_source(state: &mut ProjectState) -> anyhow::Result<()> {
    let workspace = state.workspace.clone();
    let workspace_lowering_config = state.workspace_lowering_config.clone();
    let cargo_metadata_config = state.cargo_metadata_config.clone();
    let body_ir_policy = state.body_ir_policy;
    let split_indexing_mode = state.split_indexing_mode;
    let indexing_preference = state.indexing_preference;
    let package_residency_policy = state.package_residency_policy;
    let cache_instance = state.cache_instance.clone();
    let memory_hooks = Arc::clone(&state.memory_hooks);
    let mut memory_sampler = BuildMemorySampler::disabled();

    // Keep recovery in the original cache namespace. The environment that selected the target
    // directory may have changed since the project was opened.
    let rebuilt = build::build_resident_state(
        workspace,
        workspace_lowering_config,
        cargo_metadata_config,
        cache_instance,
        body_ir_policy,
        split_indexing_mode,
        indexing_preference,
        package_residency_policy,
        StartupCacheLoad::Disabled,
        memory_hooks,
        &mut memory_sampler,
    )
    .context("while attempting to rebuild resident analysis project")?;

    *state = rebuilt;

    Ok(())
}
