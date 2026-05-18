use std::{io::Write as _, path::PathBuf, time::Instant};

use anyhow::Context as _;
use clap::ValueEnum;
use rg_lsp_engine::MemoryControl as _;
use rg_project::{
    BuildProcessMemory, BuildProfileStage, PackageResidencyPolicy, Project, StartupCacheLoad,
};
use rg_workspace::{CargoMetadataConfig, SysrootSources, WorkspaceMetadata};

mod fmt;
mod report;

/// CLI-facing package residency names for the `analyze` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliPackageResidencyPolicy {
    AllResident,
    Workspace,
    WorkspaceAndPathDeps,
    WorkspacePathAndDirectDeps,
    AllOffloadable,
}

impl From<CliPackageResidencyPolicy> for PackageResidencyPolicy {
    fn from(policy: CliPackageResidencyPolicy) -> Self {
        match policy {
            CliPackageResidencyPolicy::AllResident => Self::AllResident,
            CliPackageResidencyPolicy::Workspace => Self::WorkspaceResident,
            CliPackageResidencyPolicy::WorkspaceAndPathDeps => Self::WorkspaceAndPathDepsResident,
            CliPackageResidencyPolicy::WorkspacePathAndDirectDeps => {
                Self::WorkspacePathAndDirectDepsResident
            }
            CliPackageResidencyPolicy::AllOffloadable => Self::AllOffloadable,
        }
    }
}

/// Output format for the `analyze` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum OutputFormat {
    Text,
    Json,
}

/// Build stage used for detailed retained-memory reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliMemoryStage {
    #[value(alias = "parse")]
    Parse,
    #[value(alias = "cacheprobe")]
    CacheProbe,
    #[value(alias = "itemtree")]
    ItemTree,
    #[value(alias = "itemtree-syntax-eviction")]
    ItemTreeSyntaxEviction,
    #[value(alias = "cache-source-fingerprint")]
    CacheSourceFingerprints,
    #[value(alias = "defmap")]
    DefMap,
    #[value(alias = "semanticir")]
    SemanticIr,
    #[value(alias = "itemtree-drop")]
    ItemTreeDrop,
    #[value(alias = "bodyir")]
    BodyIr,
    #[value(alias = "parse-syntax-evict")]
    ParseSyntaxEviction,
    Final,
}

impl CliMemoryStage {
    fn build_stage(self) -> Option<BuildProfileStage> {
        match self {
            Self::Parse => Some(BuildProfileStage::Parse),
            Self::CacheProbe => Some(BuildProfileStage::CacheProbe),
            Self::ItemTree => Some(BuildProfileStage::ItemTree),
            Self::ItemTreeSyntaxEviction => Some(BuildProfileStage::ItemTreeSyntaxEviction),
            Self::CacheSourceFingerprints => Some(BuildProfileStage::CacheSourceFingerprints),
            Self::DefMap => Some(BuildProfileStage::DefMap),
            Self::SemanticIr => Some(BuildProfileStage::SemanticIr),
            Self::ItemTreeDrop => Some(BuildProfileStage::ItemTreeDrop),
            Self::BodyIr => Some(BuildProfileStage::BodyIr),
            Self::ParseSyntaxEviction => Some(BuildProfileStage::ParseSyntaxEviction),
            Self::Final => None,
        }
    }
}

/// Runs project analysis for the Cargo manifest at `path` and prints a small build summary.
#[allow(clippy::too_many_arguments)]
pub(crate) fn analyze(
    path: PathBuf,
    profile: bool,
    include_memory: bool,
    startup_cache_load: StartupCacheLoad,
    package_residency_policy: PackageResidencyPolicy,
    target: Option<String>,
    output_format: OutputFormat,
    memory_stage: CliMemoryStage,
) -> anyhow::Result<()> {
    if !path.exists() {
        anyhow::bail!("folder {} does not exist", path.display());
    }

    let cargo_manifest = path.join("Cargo.toml");
    if !cargo_manifest.exists() {
        anyhow::bail!("folder {} does not have Cargo.toml in it", path.display());
    }

    let cargo_metadata_config = target
        .map(|target| CargoMetadataConfig::default().target_triple(target))
        .unwrap_or_default();
    let metadata_started = Instant::now();
    let metadata = cargo_metadata_config
        .load_metadata(&cargo_manifest)
        .context("cargo metadata failed")?;
    let metadata_elapsed = metadata_started.elapsed();

    let workspace_started = Instant::now();
    let workspace = WorkspaceMetadata::from_cargo(metadata)
        .context("while attempting to normalize Cargo metadata")?;
    let workspace_elapsed = workspace_started.elapsed();

    let sysroot_started = Instant::now();
    let sysroot = SysrootSources::discover(workspace.workspace_root());
    let sysroot_elapsed = sysroot_started.elapsed();
    let workspace = workspace.with_sysroot_sources(sysroot);
    let memory_control = crate::memory::memory_control();
    let analysis_setup =
        report::AnalysisSetupReport::new(metadata_elapsed, workspace_elapsed, sysroot_elapsed);

    let builder = Project::builder(workspace)
        .cargo_metadata_config(cargo_metadata_config)
        .package_residency_policy(package_residency_policy)
        .startup_cache_load(startup_cache_load)
        .profile_build_timing(profile || include_memory)
        .stage_memory_target(include_memory.then(|| memory_stage.build_stage()).flatten());
    let builder = if include_memory {
        builder
            .memory_hooks(crate::memory::project_memory_hooks())
            .measure_retained_memory(true)
            .process_memory_sampler(move || {
                memory_control
                    .allocator_stats()
                    .map(|stats| BuildProcessMemory {
                        allocated_bytes: stats.allocated_bytes,
                        active_bytes: stats.active_bytes,
                        resident_bytes: stats.resident_bytes,
                    })
            })
    } else {
        builder
    };
    let project_build = builder.build().context(if include_memory {
        "while attempting to build profiled project"
    } else {
        "while attempting to build project"
    })?;

    let (project, build_profile) = project_build.into_parts();
    let allocator_name = memory_control.allocator_name();

    // Capture allocator stats and purge after project-building allocations are finished, but
    // before memory aggregation or text/JSON rendering can allocate and perturb the measurements.
    let allocator_stats = include_memory
        .then(|| memory_control.allocator_stats())
        .flatten();
    let purge = include_memory
        .then(|| report::AllocatorPurgeReport::purge_memory_and_collect(&memory_control))
        .flatten();
    let allocator = include_memory
        .then(|| report::AllocatorReport::capture(allocator_name, allocator_stats, purge));
    let report = report::AnalyzeReport::build(
        &project,
        analysis_setup,
        build_profile.as_ref(),
        allocator,
        include_memory,
        memory_stage,
    );

    let output = match output_format {
        OutputFormat::Text => {
            let mut output = String::new();
            report
                .render_text(
                    fmt::TextRenderOptions {
                        include_profile: profile,
                        include_memory,
                    },
                    &mut output,
                )
                .expect("writing to a string should not fail");
            output
        }
        OutputFormat::Json => {
            let mut output = report
                .render_json()
                .context("while attempting to render analyze JSON report")?;
            output.push('\n');
            output
        }
    };
    std::io::stdout()
        .lock()
        .write_all(output.as_bytes())
        .context("while attempting to write analyze report")?;

    Ok(())
}
