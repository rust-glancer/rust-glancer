//! Public-LSP comparison harness.
//!
//! The command starts `rust-glancer lsp` and a reference server, opens the same fixture files in
//! both, then sends a static query vector through the public LSP boundary. Raw responses stay
//! attached to the server that produced them so downstream normalization can compare behavior
//! without mixing protocol setup with report logic.

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

pub(crate) use self::config::{CliFixture, OutputFormat};
use self::fixture::Fixture;

pub(crate) async fn run(
    fixture: CliFixture,
    path: Option<PathBuf>,
    output_format: OutputFormat,
) -> anyhow::Result<()> {
    let fixture = Fixture::resolve(fixture, path)?;
    let mut servers = server::StartedServers::start(&fixture).await?;
    let opened_files = servers.opened_files();
    let rust_glancer = report::ServerReport::capture(
        servers.rust_glancer_readiness().name(),
        servers.rust_glancer_command_label(),
        servers.rust_glancer_readiness().initialize_latency(),
        servers.rust_glancer_readiness().ready_latency(),
    );
    let rust_analyzer = report::ServerReport::capture(
        servers.rust_analyzer_readiness().name(),
        servers.rust_analyzer_command_label(),
        servers.rust_analyzer_readiness().initialize_latency(),
        servers.rust_analyzer_readiness().ready_latency(),
    );
    let execution = execution::run(&fixture, &mut servers).await;

    // Query execution may fail after both processes have accepted initialization. Try graceful
    // shutdown before surfacing that error so the command does not leave the peer server running.
    let shutdown = servers.shutdown().await;
    let execution = execution?;
    shutdown?;
    let normalized = normalization::NormalizedSummary::from_execution(&fixture, &execution)?;
    let comparison = comparison::ComparisonSummary::from_normalized(&normalized);
    let report = report::LspComparisonReport::build(
        &fixture,
        opened_files,
        rust_glancer,
        rust_analyzer,
        &comparison,
    );

    output::write_report(&report, output_format)
}
