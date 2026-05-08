//! Fresh project construction.

mod cache_probe;
mod phases;

use anyhow::Context as _;

use rg_body_ir::BodyIrBuildPolicy;
use rg_workspace::{CargoMetadataConfig, WorkspaceMetadata};

use crate::{
    BuildProcessMemory, BuildProfile, PackageResidencyPlan, PackageResidencyPolicy,
    cache::{PackageCacheStore, WorkspaceCachePlan},
    profile::{BuildProfiler, ProcessMemorySampler},
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

/// Result of building a project, optionally including build-time profiling data.
pub struct ProjectBuild {
    project: Project,
    profile: Option<BuildProfile>,
}

impl ProjectBuild {
    pub fn into_project(self) -> Project {
        self.project
    }

    pub fn profile(&self) -> Option<&BuildProfile> {
        self.profile.as_ref()
    }

    pub fn into_parts(self) -> (Project, Option<BuildProfile>) {
        (self.project, self.profile)
    }
}

/// Fluent construction API for a fresh analysis project.
pub struct ProjectBuilder {
    workspace: WorkspaceMetadata,
    cargo_metadata_config: CargoMetadataConfig,
    body_ir_policy: BodyIrBuildPolicy,
    profile_build_timing: bool,
    package_residency_policy: PackageResidencyPolicy,
    startup_cache_load: StartupCacheLoad,
    measure_retained_memory: bool,
    process_memory_sampler: Option<ProcessMemorySampler>,
}

impl ProjectBuilder {
    pub(crate) fn new(workspace: WorkspaceMetadata) -> Self {
        Self {
            workspace,
            cargo_metadata_config: CargoMetadataConfig::default(),
            body_ir_policy: BodyIrBuildPolicy::default(),
            profile_build_timing: false,
            package_residency_policy: PackageResidencyPolicy::default(),
            startup_cache_load: StartupCacheLoad::default(),
            measure_retained_memory: false,
            process_memory_sampler: None,
        }
    }

    pub fn body_ir_policy(mut self, policy: BodyIrBuildPolicy) -> Self {
        self.body_ir_policy = policy;
        self
    }

    pub fn cargo_metadata_config(mut self, config: CargoMetadataConfig) -> Self {
        self.cargo_metadata_config = config;
        self
    }

    pub fn profile_build_timing(mut self, enabled: bool) -> Self {
        self.profile_build_timing = enabled;
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

    pub fn measure_retained_memory(mut self, enabled: bool) -> Self {
        self.measure_retained_memory = enabled;
        self
    }

    pub fn process_memory_sampler(
        mut self,
        sampler: impl FnMut() -> Option<BuildProcessMemory> + 'static,
    ) -> Self {
        self.process_memory_sampler = Some(Box::new(sampler));
        self
    }

    pub fn build(self) -> anyhow::Result<ProjectBuild> {
        let profile_requested = self.profile_build_timing
            || self.measure_retained_memory
            || self.process_memory_sampler.is_some();
        let mut profiler = BuildProfiler::new(
            self.profile_build_timing,
            self.measure_retained_memory,
            self.process_memory_sampler,
        );
        let mut state = build_resident_state(
            self.workspace,
            self.cargo_metadata_config,
            self.body_ir_policy,
            self.package_residency_policy,
            self.startup_cache_load,
            &mut profiler,
        )
        .context("while attempting to build resident analysis project")?;
        ResidencyApplication::fresh(&mut state)
            .apply()
            .context("while attempting to apply package cache residency")?;

        let process_memory = profiler.sample_process_memory();
        let project_bytes = profiler.measure(&state);
        profiler.record(
            "after project",
            project_bytes,
            project_bytes,
            process_memory,
        );
        let profile = profile_requested.then(|| profiler.finish());

        Ok(ProjectBuild {
            project: Project { state },
            profile,
        })
    }
}

pub(crate) fn build_resident_state(
    workspace: WorkspaceMetadata,
    cargo_metadata_config: CargoMetadataConfig,
    body_ir_policy: BodyIrBuildPolicy,
    package_residency_policy: PackageResidencyPolicy,
    startup_cache_load: StartupCacheLoad,
    profiler: &mut BuildProfiler,
) -> anyhow::Result<ProjectState> {
    let package_residency = PackageResidencyPlan::build(&workspace, package_residency_policy);
    let cache_plan = WorkspaceCachePlan::build(&workspace);
    let cache_store = PackageCacheStore::for_workspace(&workspace, &cache_plan);
    let phases = phases::build(
        &workspace,
        body_ir_policy,
        &package_residency,
        &cache_plan,
        &cache_store,
        startup_cache_load,
        profiler,
    )?;

    Ok(ProjectState {
        workspace,
        cargo_metadata_config,
        cache_plan,
        cache_store,
        package_source_fingerprints: phases.package_source_fingerprints,
        body_ir_policy,
        package_residency_policy,
        package_residency,
        names: phases.names,
        parse: phases.parse,
        def_map: phases.def_map,
        semantic_ir: phases.semantic_ir,
        body_ir: phases.body_ir,
    })
}
