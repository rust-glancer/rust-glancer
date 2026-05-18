//! Rebuilds selected packages inside an existing project snapshot.

use std::sync::Arc;

use anyhow::Context as _;

use rg_body_ir::BodyIrFile;
use rg_def_map::PackageSlot;
use rg_item_tree::ItemTreeDb;

use crate::{
    ProjectMemoryPurgePoint,
    profile::BuildProfiler,
    project::{
        StartupCacheLoad, build, loading::PackageReadLoaders, offloading::ResidencyApplication,
        state::ProjectState, subset,
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
    let rebuild_subset = subset::rebuild_packages_with_visible_dependencies(
        &state.workspace,
        plan.semantic_packages,
    );
    let loaders = PackageReadLoaders::new(state);
    let old_def_map_txn = state
        .def_map
        .read_txn_for_subset(loaders.def_map.clone(), &rebuild_subset);

    let package_indices = plan
        .semantic_packages
        .iter()
        .map(|package| package.0)
        .collect::<Vec<_>>();
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

    let def_map = state
        .def_map
        .package_rebuilder(
            &old_def_map_txn,
            &state.workspace,
            &state.parse,
            &item_tree,
            plan.semantic_packages,
            &mut state.names,
        )
        .build()
        .context("while attempting to rebuild affected def-map packages")?;
    drop(old_def_map_txn);
    let semantic_ir = state
        .semantic_ir
        .package_rebuilder(
            &item_tree,
            &def_map,
            plan.semantic_packages,
            loaders.def_map.clone(),
            loaders.semantic_ir.clone(),
            &rebuild_subset,
        )
        .build()
        .context("while attempting to rebuild affected semantic IR packages")?;
    let body_packages = plan.body_packages();
    let mut body_rebuilder = state.body_ir.package_rebuilder(
        &state.parse,
        &def_map,
        &semantic_ir,
        &body_packages,
        &mut state.names,
        loaders.def_map,
        loaders.semantic_ir,
        &rebuild_subset,
    );
    body_rebuilder = match plan.body_scope {
        BodyRebuildScope::SavedPolicy => body_rebuilder.policy(state.body_ir_policy),
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
    state.names.shrink_to_fit();
    if matches!(plan.residency, RebuildResidency::RestoreSavedState) {
        ResidencyApplication::restore(state, plan.semantic_packages)
            .apply()
            .context("while attempting to apply package cache residency after package rebuild")?;
    }

    Ok(())
}

struct PackageRebuildPlan<'a> {
    semantic_packages: &'a [PackageSlot],
    body_scope: BodyRebuildScope<'a>,
    residency: RebuildResidency,
}

impl<'a> PackageRebuildPlan<'a> {
    fn saved(packages: &'a [PackageSlot]) -> Self {
        Self {
            semantic_packages: packages,
            body_scope: BodyRebuildScope::SavedPolicy,
            residency: RebuildResidency::RestoreSavedState,
        }
    }

    fn dirty_overlay(semantic_packages: &'a [PackageSlot], body_files: &'a [BodyIrFile]) -> Self {
        Self {
            semantic_packages,
            body_scope: BodyRebuildScope::DirtyFiles(body_files),
            residency: RebuildResidency::KeepResident,
        }
    }

    fn body_packages(&self) -> Vec<PackageSlot> {
        match self.body_scope {
            BodyRebuildScope::SavedPolicy => self.semantic_packages.to_vec(),
            BodyRebuildScope::DirtyFiles(files) => {
                let mut packages = files.iter().map(|file| file.package).collect::<Vec<_>>();
                packages.sort_by_key(|package| package.0);
                packages.dedup();
                packages
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyRebuildScope<'a> {
    SavedPolicy,
    DirtyFiles(&'a [BodyIrFile]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RebuildResidency {
    RestoreSavedState,
    KeepResident,
}

pub(crate) fn rebuild_resident_from_source(state: &mut ProjectState) -> anyhow::Result<()> {
    let workspace = state.workspace.clone();
    let cargo_metadata_config = state.cargo_metadata_config.clone();
    let body_ir_policy = state.body_ir_policy;
    let package_residency_policy = state.package_residency_policy;
    let cache_store = state.cache_store.clone();
    let memory_hooks = Arc::clone(&state.memory_hooks);
    let mut profiler = BuildProfiler::disabled();
    let mut rebuilt = build::build_resident_state(
        workspace,
        cargo_metadata_config,
        body_ir_policy,
        package_residency_policy,
        StartupCacheLoad::Disabled,
        memory_hooks,
        &mut profiler,
    )
    .context("while attempting to rebuild resident analysis project")?;

    // Keep the original cache namespace. Recovery can happen while the process is alive, and the
    // environment that selected the target directory may have changed since initialization.
    rebuilt.cache_store = cache_store;
    *state = rebuilt;

    Ok(())
}
