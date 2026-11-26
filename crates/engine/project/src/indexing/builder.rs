//! Configure and construct a saved project.

use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use rg_def_map::CrateDependencies;
use rg_std::MemorySize as _;
use rg_workspace::{CargoMetadataConfig, WorkspaceLoweringConfig, WorkspaceMetadata};

use super::build_state;
use crate::{
    BuildProcessMemory, ProcessMemorySampler, Project, ProjectMemoryHooks, ProjectMemoryPurgePoint,
    memory::NoopProjectMemoryHooks, profile::record_build_checkpoint,
    project_queries::ProjectQueries,
};

pub struct ProjectBuilder {
    workspace: WorkspaceMetadata,
    workspace_lowering_config: WorkspaceLoweringConfig,
    cargo_metadata_config: CargoMetadataConfig,
    reuse_cache: bool,
    process_memory_sampler: Option<ProcessMemorySampler>,
    memory_hooks: Arc<dyn ProjectMemoryHooks>,
}

impl ProjectBuilder {
    pub(crate) fn new(workspace: WorkspaceMetadata) -> Self {
        Self {
            workspace,
            workspace_lowering_config: WorkspaceLoweringConfig::default(),
            cargo_metadata_config: CargoMetadataConfig::default(),
            reuse_cache: true,
            process_memory_sampler: None,
            memory_hooks: Arc::new(NoopProjectMemoryHooks),
        }
    }

    pub fn cargo_metadata_config(mut self, config: CargoMetadataConfig) -> Self {
        self.cargo_metadata_config = config;
        self
    }

    pub fn workspace_lowering_config(mut self, config: WorkspaceLoweringConfig) -> Self {
        self.workspace_lowering_config = config;
        self
    }

    /// Analyze from source while keeping the cache available for subsequent starts.
    pub fn reuse_cache(mut self, enabled: bool) -> Self {
        self.reuse_cache = enabled;
        self
    }

    pub fn process_memory_sampler(
        mut self,
        sampler: impl FnMut() -> Option<BuildProcessMemory> + 'static,
    ) -> Self {
        self.process_memory_sampler = Some(Box::new(sampler));
        self
    }

    pub fn memory_hooks(mut self, hooks: Arc<dyn ProjectMemoryHooks>) -> Self {
        self.memory_hooks = hooks;
        self
    }

    pub fn build(mut self) -> anyhow::Result<Project> {
        let mut queries = ProjectQueries::open(&self.workspace, self.reuse_cache)
            .context("open project query storage")?;
        tracing::debug!("registering saved project inputs");
        let state = queries
            .update_saved(|database| {
                let targets = database.update_inputs(|update| {
                    self.workspace.write_inputs(update)?;
                    CrateDependencies::write_inputs(&self.workspace, update)
                })?;
                Ok(build_state(database, self.workspace, targets)?)
            })
            .context("index saved project")?;
        let project = Project {
            queries: Mutex::new(queries),
            state,
            workspace_lowering_config: self.workspace_lowering_config,
            cargo_metadata_config: self.cargo_metadata_config,
            memory_hooks: self.memory_hooks,
        };
        project.collect_at(ProjectMemoryPurgePoint::AfterProjectBuild);
        let bytes = self
            .process_memory_sampler
            .as_ref()
            .map(|_| project.memory_size());
        let process_memory = self
            .process_memory_sampler
            .as_mut()
            .and_then(|sampler| sampler());
        record_build_checkpoint("after project", bytes, process_memory);
        Ok(project)
    }
}
