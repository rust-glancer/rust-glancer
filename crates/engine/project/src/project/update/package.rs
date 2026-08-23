//! Rebuilds selected packages inside an existing project snapshot.
//!
//! The candidate first resets each selected package to its Cargo roots, then rediscovers ordinary
//! and macro-generated module files. Its source inventory is sealed only after that complete file
//! set has converged.

use std::sync::Arc;

use anyhow::Context as _;

use rg_def_map::PackageSlot;
use rg_item_tree::ItemTreeDb;
use rg_std::Shrink;

use crate::{
    ProjectMemoryPurgePoint,
    profile::BuildMemorySampler,
    project::{
        SplitIndexingMode, StartupCacheLoad, build, generated_modules, loading::PackageReadLoaders,
        offloading::ResidencyApplication, package_set::PhasePackageSet, state::ProjectState,
        stats::MacroExpansionLimitBuildSummary,
    },
};

pub(super) fn rebuild_packages(
    state: &mut ProjectState,
    packages: &[PackageSlot],
) -> anyhow::Result<()> {
    if packages.is_empty() {
        return Ok(());
    }

    match try_rebuild_packages(state, packages) {
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

fn try_rebuild_packages(state: &mut ProjectState, packages: &[PackageSlot]) -> anyhow::Result<()> {
    let packages = PhasePackageSet::from_slice(packages);
    // Rebuilding one package can resolve names through its dependencies, but unrelated packages
    // should stay offloaded so save handling does not recreate full-project spikes.
    let rebuild_subset = packages.visible_dependency_subset(&state.workspace);
    let package_indices = packages.package_indices();

    // Replace the package file table before discovering modules again. Keeping the old table would
    // make removed modules permanent members of source validation and cache snapshots even after
    // the new ItemTree stopped reaching them.
    state
        .parse
        .reset_packages_from_workspace(&state.workspace, &package_indices)
        .context("while attempting to reset rebuilt package source roots")?;

    let loaders = PackageReadLoaders::for_package_rebuild(state, packages.as_slice());
    let old_def_map_txn = state
        .def_map
        .read_txn_for_subset(loaders.def_map.clone(), &rebuild_subset);

    let mut item_tree =
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
    // workspace at once. Saved package rebuilds are usually smaller, so avoid adding extra
    // def-map/body purges to this path.
    // Keep Parse and ItemTree mutable until generated-module requests stop adding files. This uses
    // the same coordinator as fresh construction, while clean dependency packages stay lazy.
    let memory_hooks = Arc::clone(&state.memory_hooks);
    let def_map = generated_modules::rebuild_packages(
        &state.def_map,
        &old_def_map_txn,
        &state.workspace,
        &mut state.parse,
        &mut item_tree,
        &packages,
        &mut state.names,
        state.indexing_preference.macro_expansion_preference(),
        memory_hooks.as_ref(),
    )
    .context("while attempting to rebuild affected def-map packages")?;
    drop(old_def_map_txn);
    // The selected package file tables now contain only sources reachable from this rebuild,
    // including late generated modules and excluding generated paths that disappeared.
    state.parse.seal_sources();
    let macro_expansion_limit_summary =
        MacroExpansionLimitBuildSummary::capture(&def_map, packages.as_slice());
    let semantic_ir = state
        .semantic_ir
        .package_rebuilder(
            &item_tree,
            &def_map,
            packages.as_slice(),
            loaders.def_map.clone(),
            loaders.semantic_ir.clone(),
            &rebuild_subset,
        )
        .build()
        .context("while attempting to rebuild affected semantic IR packages")?;

    let body_rebuilder = state
        .body_ir
        .package_rebuilder(
            &state.parse,
            &def_map,
            &semantic_ir,
            packages.as_slice(),
            &mut state.names,
            loaders.def_map.clone(),
            loaders.semantic_ir.clone(),
            &rebuild_subset,
        )
        .worker_limit(state.indexing_preference.body_ir_worker_limit());
    let body_rebuilder = match state.split_indexing_mode {
        SplitIndexingMode::Full => body_rebuilder.configured_bodies(state.body_ir_policy),
        SplitIndexingMode::EarlyStart => body_rebuilder.coverage_only(state.body_ir_policy),
    };
    let body_ir = body_rebuilder
        .build()
        .context("while attempting to rebuild affected body IR packages")?;
    // Validate every late read and missing-path probe before the candidate replaces retained state.
    state
        .parse
        .validate_saved_sources()
        .context("while attempting to validate captured project source generation")?;
    state.parse.evict_saved_source_text();

    // ItemTree is a transient rebuild input. Drop it before pruning the weak interner so names
    // that did not survive into retained DBs are no longer treated as live.
    drop(item_tree);

    state.macro_expansion_limit_summary = macro_expansion_limit_summary;
    state.def_map = def_map;
    state.semantic_ir = semantic_ir;
    state.body_ir = body_ir;
    Shrink::shrink_to_fit(&mut state.names);
    ResidencyApplication::restore(state, packages.as_slice())
        .apply()
        .context("while attempting to apply package cache residency after package rebuild")?;

    Ok(())
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
