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
        StartupCacheLoad, build, loading::PackageReadLoaders, offloading::ResidencyApplication,
        package_set::PhasePackageSet, state::ProjectState,
    },
};

pub(super) fn rebuild_packages(
    state: &mut ProjectState,
    packages: &[PackageSlot],
) -> anyhow::Result<()> {
    if packages.is_empty() {
        return Ok(());
    }

    let plan = PackageRebuildPlan::saved(packages);
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

pub(super) fn rebuild_dirty_overlay_packages(
    state: &mut ProjectState,
    packages: &[PackageSlot],
    body_files: &[BodyIrFile],
) -> anyhow::Result<()> {
    if packages.is_empty() {
        return Ok(());
    }

    try_rebuild_packages(
        state,
        PackageRebuildPlan::dirty_overlay(packages, body_files),
    )?;
    state
        .memory_hooks
        .purge(ProjectMemoryPurgePoint::AfterDirtyOverlayBuild);
    Ok(())
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
    let loaders = PackageReadLoaders::new(state);
    let old_def_map_txn = state
        .def_map
        .read_txn_for_subset(loaders.def_map.clone(), &rebuild_subset);

    let package_indices = plan.source_packages.package_indices();
    let item_tree =
        ItemTreeDb::build_packages(&mut state.parse, &package_indices, &mut state.names)
            .context("while attempting to rebuild affected item-tree packages")?;

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
    let mut body_rebuilder = state.body_ir.package_rebuilder(
        &state.parse,
        &def_map,
        &semantic_ir,
        plan.body_packages.as_slice(),
        &mut state.names,
        loaders.def_map,
        loaders.semantic_ir,
        &rebuild_subset,
    );
    body_rebuilder = match plan.body_scope {
        BodyRebuildScope::Policy => body_rebuilder.policy(state.body_ir_policy),
        BodyRebuildScope::DirtyFiles(files) => body_rebuilder.selected_files(files.to_vec()),
    };
    let body_ir = body_rebuilder
        .build()
        .context("while attempting to rebuild affected body IR packages")?;

    // ItemTree is a transient rebuild input. Drop it before pruning the weak interner so names
    // that did not survive into retained DBs are no longer treated as live.
    drop(item_tree);

    state.def_map = def_map;
    state.semantic_ir = semantic_ir;
    state.body_ir = body_ir;
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
}

impl<'a> PackageRebuildPlan<'a> {
    fn saved(packages: &'a [PackageSlot]) -> Self {
        Self {
            source_packages: PhasePackageSet::from_slice(packages),
            body_packages: PhasePackageSet::from_slice(packages),
            body_scope: BodyRebuildScope::Policy,
            residency: RebuildResidency::RestoreSavedState,
        }
    }

    fn dirty_overlay(source_packages: &'a [PackageSlot], body_files: &'a [BodyIrFile]) -> Self {
        Self {
            source_packages: PhasePackageSet::from_slice(source_packages),
            body_packages: PhasePackageSet::from_body_files(body_files),
            body_scope: BodyRebuildScope::DirtyFiles(body_files),
            residency: RebuildResidency::KeepResident,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyRebuildScope<'a> {
    Policy,
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
