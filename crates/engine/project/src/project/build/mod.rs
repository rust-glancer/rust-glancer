//! Fresh project construction.

mod cache_probe;
mod checkpoint_memory;
mod phases;

use anyhow::Context as _;
use std::sync::Arc;

use rg_body_ir::BodyIrBuildPolicy;
use rg_workspace::{CargoMetadataConfig, WorkspaceLoweringConfig, WorkspaceMetadata};

use crate::{
    BuildProcessMemory, IndexingPerformancePreference, PackageResidencyPlan,
    PackageResidencyPolicy, ProjectMemoryHooks, ProjectMemoryPurgePoint,
    cache::{PackageCacheInstance, PackageCacheStore, WorkspaceCachePlan},
    memory::NoopProjectMemoryHooks,
    profile::{BuildMemorySampler, record_build_checkpoint},
};

use super::{Project, offloading::ResidencyApplication, state::ProjectState};

/// Controls whether a fresh project build can seed offloadable packages from cache artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartupCacheLoad {
    Disabled,
    #[default]
    Enabled,
}

impl StartupCacheLoad {
    pub(crate) fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

/// Fluent construction API for a fresh analysis project.
pub struct ProjectBuilder {
    workspace: WorkspaceMetadata,
    workspace_lowering_config: WorkspaceLoweringConfig,
    cargo_metadata_config: CargoMetadataConfig,
    body_ir_policy: BodyIrBuildPolicy,
    indexing_preference: IndexingPerformancePreference,
    package_residency_policy: PackageResidencyPolicy,
    startup_cache_load: StartupCacheLoad,
    memory_sampler: BuildMemorySampler,
    memory_hooks: Arc<dyn ProjectMemoryHooks>,
}

impl ProjectBuilder {
    pub(crate) fn new(workspace: WorkspaceMetadata) -> Self {
        Self {
            workspace,
            workspace_lowering_config: WorkspaceLoweringConfig::default(),
            cargo_metadata_config: CargoMetadataConfig::default(),
            body_ir_policy: BodyIrBuildPolicy::default(),
            indexing_preference: IndexingPerformancePreference::default(),
            package_residency_policy: PackageResidencyPolicy::default(),
            startup_cache_load: StartupCacheLoad::default(),
            memory_sampler: BuildMemorySampler::disabled(),
            memory_hooks: Arc::new(NoopProjectMemoryHooks),
        }
    }

    pub fn body_ir_policy(mut self, policy: BodyIrBuildPolicy) -> Self {
        self.body_ir_policy = policy;
        self
    }

    pub fn indexing_preference(mut self, preference: IndexingPerformancePreference) -> Self {
        self.indexing_preference = preference;
        self
    }

    pub fn cargo_metadata_config(mut self, config: CargoMetadataConfig) -> Self {
        self.cargo_metadata_config = config;
        self
    }

    pub fn workspace_lowering_config(mut self, config: WorkspaceLoweringConfig) -> Self {
        self.workspace_lowering_config = config;
        self
    }

    pub fn package_residency_policy(mut self, policy: PackageResidencyPolicy) -> Self {
        self.package_residency_policy = policy;
        self
    }

    pub fn startup_cache_load(mut self, load: StartupCacheLoad) -> Self {
        self.startup_cache_load = load;
        self
    }

    /// Enables measuring retained memory for stages (via internal memory profiler).
    pub fn measure_retained_memory(mut self, enabled: bool) -> Self {
        self.memory_sampler = self.memory_sampler.with_retained_memory(enabled);
        self
    }

    /// Enables measuring BOTH retained and process memory.
    pub fn process_memory_sampler(
        mut self,
        sampler: impl FnMut() -> Option<BuildProcessMemory> + 'static,
    ) -> Self {
        self.memory_sampler = self.memory_sampler.with_process_memory(Box::new(sampler));
        self
    }

    pub fn memory_hooks(mut self, hooks: Arc<dyn ProjectMemoryHooks>) -> Self {
        self.memory_hooks = hooks;
        self
    }

    pub fn build(self) -> anyhow::Result<Project> {
        let mut memory_sampler = self.memory_sampler;
        // Claim an instance before startup probing so all cache reads and writes belong to this
        // project/LSP owner.
        let cache_instance = PackageCacheInstance::for_workspace(&self.workspace)
            .context("while attempting to claim package cache instance")?;
        let mut state = build_resident_state(
            self.workspace,
            self.workspace_lowering_config,
            self.cargo_metadata_config,
            cache_instance,
            self.body_ir_policy,
            self.indexing_preference,
            self.package_residency_policy,
            self.startup_cache_load,
            Arc::clone(&self.memory_hooks),
            &mut memory_sampler,
        )
        .context("while attempting to build resident analysis project")?;
        ResidencyApplication::fresh(&mut state)
            .apply_profiled(&mut memory_sampler)
            .context("while attempting to apply package cache residency")?;
        self.memory_hooks
            .purge(ProjectMemoryPurgePoint::AfterProjectBuild);

        let process_memory = memory_sampler.sample_process_memory();
        let project_bytes = memory_sampler.measure_retained(&state);
        record_build_checkpoint(
            "after project",
            project_bytes,
            project_bytes,
            process_memory,
        );
        Ok(Project { state })
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_resident_state(
    workspace: WorkspaceMetadata,
    workspace_lowering_config: WorkspaceLoweringConfig,
    cargo_metadata_config: CargoMetadataConfig,
    cache_instance: PackageCacheInstance,
    body_ir_policy: BodyIrBuildPolicy,
    indexing_preference: IndexingPerformancePreference,
    package_residency_policy: PackageResidencyPolicy,
    startup_cache_load: StartupCacheLoad,
    memory_hooks: Arc<dyn ProjectMemoryHooks>,
    memory_sampler: &mut BuildMemorySampler,
) -> anyhow::Result<ProjectState> {
    let package_residency = PackageResidencyPlan::build(&workspace, package_residency_policy);
    let cache_plan = WorkspaceCachePlan::build(&workspace);
    let cache_store = PackageCacheStore::for_instance(&workspace, &cache_plan, &cache_instance);
    let phases = phases::build(
        &workspace,
        body_ir_policy,
        indexing_preference,
        &package_residency,
        &cache_plan,
        &cache_store,
        startup_cache_load,
        memory_hooks.as_ref(),
        memory_sampler,
    )?;

    Ok(ProjectState {
        workspace,
        workspace_lowering_config,
        cargo_metadata_config,
        cache_plan,
        cache_instance,
        cache_store,
        package_source_fingerprints: phases.package_source_fingerprints,
        body_ir_policy,
        indexing_preference,
        package_residency_policy,
        package_residency,
        memory_hooks,
        names: phases.names,
        parse: phases.parse,
        def_map: phases.def_map,
        semantic_ir: phases.semantic_ir,
        body_ir: phases.body_ir,
    })
}
