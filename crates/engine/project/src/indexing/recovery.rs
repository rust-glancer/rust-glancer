//! Rebuilds analysis after disposable package storage can no longer serve a coherent read.

use std::sync::Arc;

use anyhow::Context as _;
use rg_package_store::PackageStoreError;

use crate::{
    ProjectMemoryPurgePoint, StartupCacheLoad, profile::BuildMemorySampler, state::ProjectState,
    storage::residency::ResidencyApplication,
};

/// Invalidates disposable cache state, rebuilds from source, and reapplies residency.
pub(crate) fn recover_from_cache_load_failure(project: &mut ProjectState) -> anyhow::Result<()> {
    project
        .cache_store
        .clear_package_artifacts()
        .context("while attempting to clear package cache artifacts")?;
    rebuild_resident_from_source(project)
        .context("while attempting to rebuild resident analysis project from source")?;
    let memory_hooks = Arc::clone(&project.memory_hooks);
    ResidencyApplication::fresh(project)
        .apply()
        .context("while attempting to reapply package cache residency")?;
    memory_hooks.purge(ProjectMemoryPurgePoint::AfterProjectBuild);

    Ok(())
}

fn rebuild_resident_from_source(state: &mut ProjectState) -> anyhow::Result<()> {
    let workspace = state.workspace.clone();
    let workspace_lowering_config = state.workspace_lowering_config.clone();
    let cargo_metadata_config = state.cargo_metadata_config.clone();
    let body_ir_policy = state.body_ir_policy;
    let split_indexing_mode = state.split_indexing_mode;
    let indexing_preference = state.indexing_preference;
    let package_batch_size = state.package_batch_size;
    let package_residency_policy = state.package_residency_policy;
    let cache_instance = state.cache_instance.clone();
    let memory_hooks = Arc::clone(&state.memory_hooks);
    let mut memory_sampler = BuildMemorySampler::disabled();

    // Keep recovery in the original cache namespace. The environment that selected the target
    // directory may have changed since the project was opened.
    let rebuilt = super::initial::build_resident_state(
        workspace,
        workspace_lowering_config,
        cargo_metadata_config,
        cache_instance,
        body_ir_policy,
        split_indexing_mode,
        indexing_preference,
        package_batch_size,
        package_residency_policy,
        StartupCacheLoad::Disabled,
        memory_hooks,
        &mut memory_sampler,
    )
    .context("while attempting to rebuild resident analysis project")?;

    *state = rebuilt;

    Ok(())
}

impl ProjectState {
    pub(crate) fn is_recoverable_cache_load_failure(error: &anyhow::Error) -> bool {
        error.chain().any(|cause| {
            matches!(
                cause.downcast_ref::<PackageStoreError>(),
                Some(PackageStoreError::Load { .. })
            )
        })
    }
}
