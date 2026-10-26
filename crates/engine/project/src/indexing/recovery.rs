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
    let mut memory_sampler = BuildMemorySampler::disabled();
    // Keep recovery in the original cache namespace. The environment that selected the target
    // directory may have changed since the project was opened.
    let rebuilt = super::initial::build_state(
        project.workspace.clone(),
        project.workspace_lowering_config.clone(),
        project.cargo_metadata_config.clone(),
        project.cache_instance.clone(),
        project.body_ir_policy,
        project.split_indexing_mode,
        project.indexing_preference,
        project.package_batch_size,
        project.package_residency_policy,
        StartupCacheLoad::Disabled,
        Arc::clone(&project.memory_hooks),
        &mut memory_sampler,
    )
    .context("while attempting to rebuild analysis project from source")?;
    *project = rebuilt;

    ResidencyApplication::fresh(project)
        .apply()
        .context("while attempting to reapply package cache residency")?;
    project
        .memory_hooks
        .purge(ProjectMemoryPurgePoint::AfterProjectBuild);

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
