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

use self::fixture::Fixture;
pub(crate) use self::{
    benchmark::BenchmarkArgs,
    config::{CliFixture, OutputFormat},
};

pub(crate) async fn run(
    fixture: Option<CliFixture>,
    path: Option<PathBuf>,
    output_format: Option<OutputFormat>,
    benchmark: BenchmarkArgs,
) -> anyhow::Result<()> {
    let Some(fixture) = fixture else {
        return benchmark::run(benchmark, output_format).await;
    };
    let fixture = Fixture::resolve(fixture, path)?;
    let (execution, opened_files, servers) = execution::run(&fixture).await?;
    execution.write_raw_results()?;
    let normalized = normalization::NormalizedSummary::from_execution(&fixture, &execution)?;
    let comparison = comparison::ComparisonSummary::from_normalized(&normalized);
    let report = report::LspComparisonReport::build(&fixture, opened_files, servers, &comparison);

    output::write_report(&report, output_format.unwrap_or(OutputFormat::Text))
}
