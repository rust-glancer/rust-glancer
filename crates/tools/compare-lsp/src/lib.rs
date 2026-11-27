//! Public-LSP comparison harness.
//!
//! Runs editor benchmarks and compatibility sweeps through server subprocesses.

mod benchmark;
mod comparison;
mod config;
mod execution;
mod fixture;
mod lsp_client;
mod normalization;
mod output;
mod query;
mod report;
mod server;

use std::path::PathBuf;

use anyhow::Context as _;

use self::{
    benchmark::BenchmarkArgs,
    config::{CliFixture, OutputFormat},
    fixture::Fixture,
};

/// Arguments for the `compare-lsp` command.
#[derive(Debug, clap::Args)]
pub struct CompareLspArgs {
    fixture: Option<CliFixture>,
    /// Override the fixture root. Defaults to the selected fixture's configured root.
    #[clap(long, requires = "fixture")]
    path: Option<PathBuf>,
    /// Render the comparison report for humans or CI tooling.
    #[clap(long, value_enum)]
    format: Option<OutputFormat>,
    #[command(flatten)]
    benchmark: BenchmarkArgs,
}

impl CompareLspArgs {
    /// Run the selected benchmark or compatibility sweep against server subprocesses.
    pub fn run(self) -> anyhow::Result<()> {
        let Self {
            fixture,
            path,
            format,
            benchmark,
        } = self;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("while attempting to build LSP comparison Tokio runtime")?;
        runtime.block_on(async move {
            let Some(fixture) = fixture else {
                return benchmark::run(benchmark, format).await;
            };
            let fixture = Fixture::resolve(fixture, path)?;
            let (execution, opened_files, servers) = execution::run(&fixture).await?;
            execution.write_raw_results()?;
            let normalized =
                normalization::NormalizedSummary::from_execution(&fixture, &execution)?;
            let comparison = comparison::ComparisonSummary::from_normalized(&normalized);
            let report =
                report::LspComparisonReport::build(&fixture, opened_files, servers, &comparison);

            output::write_report(&report, format.unwrap_or(OutputFormat::Text))
        })
    }
}
