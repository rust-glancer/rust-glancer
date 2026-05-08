use std::{path::PathBuf, time::Instant};

use anyhow::Context as _;
use rg_lsp::MemoryControl as _;
use rg_project::{BuildProcessMemory, PackageResidencyPolicy, Project, StartupCacheLoad};
use rg_workspace::{CargoMetadataConfig, SysrootSources, WorkspaceMetadata};

mod fmt;

/// Runs project analysis for the Cargo manifest at `path` and prints a small build summary.
pub(super) fn analyze(
    path: PathBuf,
    profile: bool,
    include_memory: bool,
    startup_cache_load: StartupCacheLoad,
    package_residency_policy: PackageResidencyPolicy,
    target: Option<String>,
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
    let memory_control = crate::runtime::memory_control();

    let builder = Project::builder(workspace)
        .cargo_metadata_config(cargo_metadata_config)
        .package_residency_policy(package_residency_policy)
        .startup_cache_load(startup_cache_load)
        .profile_build_timing(profile || include_memory);
    let builder = if include_memory {
        builder
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
    self::fmt::print_project_summary(&project);
    if profile {
        self::fmt::print_analysis_setup_profile(
            metadata_elapsed,
            workspace_elapsed,
            sysroot_elapsed,
        );
    }
    if profile
        && !include_memory
        && let Some(profile) = &build_profile
    {
        self::fmt::print_build_profile(profile, None);
    }
    if include_memory {
        println!("allocator: {}", memory_control.allocator_name());
        if let Some(stats) = memory_control.allocator_stats() {
            self::fmt::print_allocator_stats(stats);
        }
        let purge = self::fmt::purge_allocator_after_build(&memory_control);
        if let Some(purge) = &purge {
            self::fmt::print_allocator_purge_after_build(purge);
        }
        if let Some(profile) = &build_profile {
            self::fmt::print_build_profile(profile, purge.as_ref());
        }
        self::fmt::print_memory_summary(&project);
    }

    Ok(())
}
