//! Configure and construct a saved project.

use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use rg_def_map::CrateDependencies;
use rg_std::MemorySize as _;
use rg_workspace::{CargoMetadataConfig, WorkspaceLoweringConfig, WorkspaceMetadata};

use super::build_state;
use crate::{
    BuildProcessMemory, ProcessMemorySampler, Project, ProjectMemoryHooks, ProjectMemoryPurgePoint,
    memory::NoopProjectMemoryHooks, profile::record_build_checkpoint, project_queries,
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
        let mut database =
            project_queries::open(&self.workspace).context("open saved query cache")?;
        if !self.reuse_cache {
            database.invalidate().context("reset saved query cache")?;
        }
        let targets = database.update_inputs(|update| {
            self.workspace.write_inputs(update)?;
            CrateDependencies::write_inputs(&self.workspace, update)
        })?;
        let state =
            build_state(&mut database, self.workspace, targets).context("index saved project")?;
        self.memory_hooks
            .purge(ProjectMemoryPurgePoint::AfterProjectBuild);

        let bytes = self
            .process_memory_sampler
            .as_ref()
            .map(|_| state.memory_size());
        let process_memory = self
            .process_memory_sampler
            .as_mut()
            .and_then(|sampler| sampler());
        record_build_checkpoint("after project", bytes, process_memory);
        Ok(Project {
            database: Mutex::new(database),
            state,
            workspace_lowering_config: self.workspace_lowering_config,
            cargo_metadata_config: self.cargo_metadata_config,
            memory_hooks: self.memory_hooks,
        })
    }
}
