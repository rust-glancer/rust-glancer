use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::Context as _;
use rg_lsp_engine::MemoryControl as _;
use rg_profile::ProfileRun;
use rg_project::{
    BuildProcessMemory, IndexingPerformancePreference, PackageResidencyPolicy, Project,
    StartupCacheLoad,
};
use rg_workspace::{CargoMetadataConfig, SysrootSources, WorkspaceMetadata};

mod config;
mod data;
mod output;
mod profile;
mod report;

pub(crate) use self::config::{CliIndexingPreference, CliPackageResidencyPolicy, OutputFormat};
pub(crate) use self::profile::profile_groups_help;

/// Runs project analysis for the Cargo manifest at `path` and prints a small build summary.
#[allow(clippy::too_many_arguments)]
pub(crate) fn analyze(
    path: PathBuf,
    profile_filter: Option<String>,
    include_memory: bool,
    startup_cache_load: StartupCacheLoad,
    package_residency_policy: PackageResidencyPolicy,
    indexing_preference: IndexingPerformancePreference,
    target: Option<String>,
    output_format: OutputFormat,
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
    let (metadata, metadata_elapsed) = measure_time(|| {
        cargo_metadata_config
            .load_metadata_with_target_cfg(&cargo_manifest)
            .context("cargo metadata failed")
    })?;

    let (workspace, workspace_elapsed) = measure_time(|| {
        WorkspaceMetadata::lower(metadata.metadata, metadata.target_cfg)
            .context("while attempting to normalize Cargo metadata")
    })?;

    let (sysroot, sysroot_elapsed) =
        measure_time(|| Ok(SysrootSources::discover(workspace.workspace_root())))?;
    let workspace: WorkspaceMetadata = workspace.with_sysroot_sources(sysroot);
    let memory_control = crate::memory::memory_control();
    let analysis_setup =
        data::AnalysisSetupReport::new(metadata_elapsed, workspace_elapsed, sysroot_elapsed);
    let include_profile_snapshot = profile_filter
        .as_deref()
        .is_some_and(|filter| !filter.trim().is_empty());
    let profile_filter = profile::parse_filter(profile_filter.as_deref(), include_memory)?;
    let profile_run = profile_filter
        .map(profile::start_run)
        .transpose()
        .context("while attempting to start analyze profile run")?;

    let builder = Project::builder(workspace)
        .cargo_metadata_config(cargo_metadata_config)
        .indexing_preference(indexing_preference)
        .package_residency_policy(package_residency_policy)
        .startup_cache_load(startup_cache_load);
    let builder = if include_memory {
        builder
            .memory_hooks(crate::memory::project_memory_hooks())
            .process_memory_sampler(move || {
                memory_control
                    .allocator_stats()
                    .map(|stats| BuildProcessMemory {
                        allocated_bytes: stats.allocated_bytes,
                        active_bytes: stats.active_bytes,
                        resident_bytes: stats.resident_bytes,
                        mapped_bytes: stats.mapped_bytes,
                    })
            })
    } else {
        builder
    };
    let project = builder
        .build()
        .context("while attempting to build project")?;
    let profile_snapshot = profile_run.map(ProfileRun::finish);

    let analyze_report = data::AnalyzeReport::build(
        &project,
        analysis_setup,
        profile_snapshot.as_ref(),
        include_profile_snapshot,
        include_memory,
    );

    output::write_report(&analyze_report, output_format, include_memory)
}

fn measure_time<T>(operation: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<(T, Duration)> {
    let started = Instant::now();
    let output = operation()?;

    Ok((output, started.elapsed()))
}

#[cfg(test)]
mod tests {
    use super::profile::{parse_filter, start_run};

    #[test]
    fn analyze_profile_filter_is_absent_without_profile_argument() {
        assert_eq!(
            parse_filter(None, false)
                .expect("missing profile filter should parse as no profile run"),
            None,
            "plain analysis should not start the dynamic profiler",
        );
    }

    #[test]
    fn analyze_profile_filter_treats_empty_selector_as_disabled() {
        assert_eq!(
            parse_filter(Some(""), false).expect("empty profile filter should parse as disabled"),
            None,
            "an explicitly empty profile selector should not start the dynamic profiler",
        );

        assert_eq!(
            parse_filter(Some("  "), false).expect("empty profile filter should parse as disabled"),
            None,
            "an explicitly empty profile selector should not start the dynamic profiler",
        );
    }

    #[test]
    fn analyze_profile_filter_enables_project_build_for_memory() {
        let project_build = rg_project::BUILD_CHECKPOINTS.scope();

        let filter = parse_filter(None, true)
            .expect("memory profile filter should parse")
            .expect("memory reporting should enable a profile run");
        assert_eq!(
            selector_texts(&filter),
            vec![project_build],
            "memory reports should collect project build checkpoints internally",
        );

        let filter = parse_filter(Some("def_map.macros"), true)
            .expect("profile filter with memory should parse")
            .expect("memory reporting should keep a profile run enabled");
        assert_eq!(
            selector_texts(&filter),
            vec!["def_map.macros", project_build],
            "memory reports should extend explicit selectors instead of replacing them",
        );

        let filter = parse_filter(Some("project.build.def_map"), true)
            .expect("detailed project build profile filter with memory should parse")
            .expect("memory reporting should keep a profile run enabled");
        assert_eq!(
            selector_texts(&filter),
            vec!["project.build.def_map"],
            "detailed project build selectors already cover parent checkpoints",
        );

        let filter = parse_filter(Some("all"), true)
            .expect("all profile filter with memory should parse")
            .expect("all profile filter should enable a profile run");
        assert!(
            filter.is_all(),
            "the all selector already includes project build checkpoints",
        );
    }

    #[test]
    fn analyze_profile_filter_expands_default_alias() {
        let final_memory = rg_project::BUILD_FINAL_MEMORY.scope();

        let filter = parse_filter(Some("default"), false)
            .expect("default profile alias should parse")
            .expect("default profile alias should enable a profile run");
        assert_eq!(
            selector_texts(&filter),
            vec![final_memory],
            "the default alias should collect build checkpoints and final retained-memory details",
        );

        let filter = parse_filter(Some("default,def_map.macros"), false)
            .expect("mixed profile aliases and selectors should parse")
            .expect("mixed profile aliases and selectors should enable a profile run");
        assert_eq!(
            selector_texts(&filter),
            vec![final_memory, "def_map.macros"],
            "profile aliases should expand without blocking explicit selectors",
        );
    }

    #[test]
    fn analyze_profile_run_accepts_registered_selectors() {
        for selector in [
            "project.build",
            "project.build.def_map",
            "def_map.macros.by_name",
            "default",
        ] {
            let filter = parse_filter(Some(selector), false)
                .expect("registered analyze profile selector should parse")
                .expect("registered analyze profile selector should enable a profile run");
            let run = start_run(filter)
                .expect("registered analyze profile selector should start a profile run");

            assert!(
                run.finish().entries().is_empty(),
                "a profile run without recorded metrics should finish with an empty snapshot",
            );
        }
    }

    #[test]
    fn analyze_profile_run_rejects_unknown_selector() {
        let filter = parse_filter(Some("def_map.unknown"), false)
            .expect("syntactically valid selector should parse")
            .expect("non-empty selector should enable a profile run");
        let error = match start_run(filter) {
            Ok(run) => {
                drop(run);
                panic!("unknown analyze profile selector should be rejected");
            }
            Err(error) => error,
        };

        assert!(
            error.chain().any(|cause| cause
                .to_string()
                .contains("profile selector `def_map.unknown` is not registered")),
            "unknown selector should fail with a typo-oriented error: {error}",
        );
    }

    fn selector_texts(filter: &rg_profile::ProfileFilter) -> Vec<&str> {
        filter.selectors().iter().map(String::as_str).collect()
    }
}
