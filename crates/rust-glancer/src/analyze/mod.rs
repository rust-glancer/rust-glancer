use std::{
    fmt as std_fmt, fs,
    io::Write as _,
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::Context as _;
use clap::ValueEnum;
use rg_lsp_engine::MemoryControl as _;
use rg_project::{
    BuildProcessMemory, IndexingPerformancePreference, PackageResidencyPolicy, Project,
    StartupCacheLoad,
};
use rg_workspace::{CargoMetadataConfig, SysrootSources, WorkspaceMetadata};

mod data;
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

/// CLI-facing indexing preference names for the `analyze` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliIndexingPreference {
    LowerPeakMemory,
    FasterBuilds,
}

impl From<CliIndexingPreference> for IndexingPerformancePreference {
    fn from(preference: CliIndexingPreference) -> Self {
        match preference {
            CliIndexingPreference::LowerPeakMemory => Self::LowerPeakMemory,
            CliIndexingPreference::FasterBuilds => Self::FasterBuilds,
        }
    }
}

impl From<IndexingPerformancePreference> for CliIndexingPreference {
    fn from(preference: IndexingPerformancePreference) -> Self {
        match preference {
            IndexingPerformancePreference::LowerPeakMemory => Self::LowerPeakMemory,
            IndexingPerformancePreference::FasterBuilds => Self::FasterBuilds,
        }
    }
}

impl Default for CliIndexingPreference {
    fn default() -> Self {
        IndexingPerformancePreference::default().into()
    }
}

impl std_fmt::Display for CliIndexingPreference {
    fn fmt(&self, f: &mut std_fmt::Formatter<'_>) -> std_fmt::Result {
        f.write_str(IndexingPerformancePreference::from(*self).config_name())
    }
}

/// Output format for the `analyze` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum OutputFormat {
    Text,
    Json,
    RichJson,
    Html,
}

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
    let metadata_started = Instant::now();
    let metadata = cargo_metadata_config
        .load_metadata_with_target_cfg(&cargo_manifest)
        .context("cargo metadata failed")?;
    let metadata_elapsed = metadata_started.elapsed();

    let workspace_started = Instant::now();
    let workspace = WorkspaceMetadata::lower(metadata.metadata, metadata.target_cfg)
        .context("while attempting to normalize Cargo metadata")?;
    let workspace_elapsed = workspace_started.elapsed();

    let sysroot_started = Instant::now();
    let sysroot = SysrootSources::discover(workspace.workspace_root());
    let sysroot_elapsed = sysroot_started.elapsed();
    let workspace = workspace.with_sysroot_sources(sysroot);
    let memory_control = crate::memory::memory_control();
    let analysis_setup =
        data::AnalysisSetupReport::new(metadata_elapsed, workspace_elapsed, sysroot_elapsed);
    let include_profile_snapshot = profile_filter
        .as_deref()
        .is_some_and(|filter| !filter.trim().is_empty());
    let profile_filter = parse_analyze_profile_filter(profile_filter.as_deref(), include_memory)?;
    let profile_run = profile_filter
        .map(start_analyze_profile_run)
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
            .measure_retained_memory(true)
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
    let project = builder.build().context(if include_memory {
        "while attempting to build profiled project"
    } else {
        "while attempting to build project"
    })?;
    let profile_snapshot = profile_run.map(rg_profile::ProfileRun::finish);

    let allocator_name = memory_control.allocator_name();

    // Capture allocator stats and purge after project-building allocations are finished, but
    // before memory aggregation or text/JSON rendering can allocate and perturb the measurements.
    let allocator_stats = include_memory
        .then(|| memory_control.allocator_stats())
        .flatten();
    let purge = include_memory
        .then(|| data::AllocatorPurgeReport::purge_memory_and_collect(&memory_control))
        .flatten();
    let allocator = include_memory
        .then(|| data::AllocatorReport::capture(allocator_name, allocator_stats, purge));
    let analyze_report = data::AnalyzeReport::build(
        &project,
        analysis_setup,
        allocator,
        profile_snapshot.as_ref(),
        include_profile_snapshot,
        include_memory,
    );

    let output = match output_format {
        OutputFormat::Text => {
            let mut output = String::new();
            let document_options = data::ReportDocumentOptions { include_memory };
            let document = analyze_report.document(document_options);
            report::TextRenderer
                .render(&document, &mut output)
                .expect("writing to a string should not fail");
            output
        }
        OutputFormat::Json => {
            let mut output = analyze_report
                .render_json()
                .context("while attempting to render analyze JSON report")?;
            output.push('\n');
            output
        }
        OutputFormat::RichJson => {
            let document_options = data::ReportDocumentOptions { include_memory };
            let document = analyze_report.document(document_options);
            let mut output = report::RichJsonRenderer
                .render(&document)
                .context("while attempting to render rich analyze JSON report")?;
            output.push('\n');
            output
        }
        OutputFormat::Html => {
            let document_options = data::ReportDocumentOptions { include_memory };
            let document = analyze_report.document(document_options);
            let path = write_html_report(&document)?;
            format!("wrote HTML report to {}\n", path.display())
        }
    };
    std::io::stdout()
        .lock()
        .write_all(output.as_bytes())
        .context("while attempting to write analyze report")?;

    Ok(())
}

fn parse_analyze_profile_filter(
    filter: Option<&str>,
    include_memory: bool,
) -> anyhow::Result<Option<rg_profile::ProfileFilter>> {
    let mut filter = match filter {
        Some(filter) => rg_profile::ProfileFilter::parse(filter)
            .context("while attempting to parse analyze profile filter")?,
        None => rg_profile::ProfileFilter::disabled(),
    };

    if include_memory {
        filter
            .enable(rg_project::BUILD_CHECKPOINTS.scope())
            .context("while attempting to enable project build profiling for memory report")?;
    }

    Ok((!filter.is_disabled()).then_some(filter))
}

fn start_analyze_profile_run(
    filter: rg_profile::ProfileFilter,
) -> anyhow::Result<rg_profile::ProfileRun> {
    let registry =
        rg_profile::ProfileRegistry::new(rg_project::profile_descriptors().iter().copied())
            .context("while attempting to build project profile registry")?;
    rg_profile::ProfileRun::start_with_registry(registry, filter)
        .context("while attempting to activate analyze profile run")
}

fn write_html_report(document: &report::ReportDocument) -> anyhow::Result<PathBuf> {
    let report_dir = PathBuf::from("target").join("rust-glancer").join("report");
    fs::create_dir_all(&report_dir).with_context(|| {
        format!(
            "while attempting to create HTML report directory {}",
            report_dir.display()
        )
    })?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("while attempting to read system time for HTML report filename")?
        .as_millis();
    let path = report_dir.join(format!("{timestamp}-report.html"));
    let html = report::HtmlRenderer.render(document);

    fs::write(&path, html).with_context(|| {
        format!(
            "while attempting to write HTML report file {}",
            path.display()
        )
    })?;

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::{parse_analyze_profile_filter, start_analyze_profile_run};

    #[test]
    fn analyze_profile_filter_is_absent_without_profile_argument() {
        assert_eq!(
            parse_analyze_profile_filter(None, false)
                .expect("missing profile filter should parse as no profile run"),
            None,
            "plain analysis should not start the dynamic profiler",
        );
    }

    #[test]
    fn analyze_profile_filter_treats_empty_selector_as_disabled() {
        assert_eq!(
            parse_analyze_profile_filter(Some(""), false)
                .expect("empty profile filter should parse as disabled"),
            None,
            "an explicitly empty profile selector should not start the dynamic profiler",
        );
    }

    #[test]
    fn analyze_profile_filter_enables_project_build_for_memory() {
        let project_build = rg_project::BUILD_CHECKPOINTS.scope();

        let filter = parse_analyze_profile_filter(None, true)
            .expect("memory profile filter should parse")
            .expect("memory reporting should enable a profile run");
        assert_eq!(
            selector_texts(&filter),
            vec![project_build],
            "memory reports should collect project build checkpoints internally",
        );

        let filter = parse_analyze_profile_filter(Some("def_map.macros"), true)
            .expect("profile filter with memory should parse")
            .expect("memory reporting should keep a profile run enabled");
        assert_eq!(
            selector_texts(&filter),
            vec!["def_map.macros", project_build],
            "memory reports should extend explicit selectors instead of replacing them",
        );

        let filter = parse_analyze_profile_filter(Some("project.build.def_map"), true)
            .expect("detailed project build profile filter with memory should parse")
            .expect("memory reporting should keep a profile run enabled");
        assert_eq!(
            selector_texts(&filter),
            vec!["project.build.def_map"],
            "detailed project build selectors already cover parent checkpoints",
        );

        let filter = parse_analyze_profile_filter(Some("all"), true)
            .expect("all profile filter with memory should parse")
            .expect("all profile filter should enable a profile run");
        assert!(
            filter.is_all(),
            "the all selector already includes project build checkpoints",
        );
    }

    #[test]
    fn analyze_profile_run_accepts_registered_selectors() {
        for selector in [
            "project.build",
            "project.build.def_map",
            "def_map.macros.by_name",
        ] {
            let filter = parse_analyze_profile_filter(Some(selector), false)
                .expect("registered analyze profile selector should parse")
                .expect("registered analyze profile selector should enable a profile run");
            let run = start_analyze_profile_run(filter)
                .expect("registered analyze profile selector should start a profile run");

            assert!(
                run.finish().entries().is_empty(),
                "a profile run without recorded metrics should finish with an empty snapshot",
            );
        }
    }

    #[test]
    fn analyze_profile_run_rejects_unknown_selector() {
        let filter = parse_analyze_profile_filter(Some("def_map.unknown"), false)
            .expect("syntactically valid selector should parse")
            .expect("non-empty selector should enable a profile run");
        let error = match start_analyze_profile_run(filter) {
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
