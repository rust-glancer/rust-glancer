//! Public-LSP comparison harness.
//!
//! The command starts `rust-glancer lsp` and a reference server, opens the same fixture files in
//! both, then sends a static query vector through the public LSP boundary. Raw responses stay
//! attached to the server that produced them so downstream normalization can compare behavior
//! without mixing protocol setup with report logic.

mod config;
mod execution;
mod fixture;
mod lsp_client;
mod normalization;
mod query;
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
    let rust_glancer_name = servers.rust_glancer_readiness().name();
    let rust_glancer_initialize = servers.rust_glancer_readiness().initialize_latency();
    let rust_analyzer_name = servers.rust_analyzer_readiness().name();
    let rust_analyzer_initialize = servers.rust_analyzer_readiness().initialize_latency();
    let execution = execution::run(&fixture, &mut servers).await;

    // Query execution may fail after both processes have accepted initialization. Try graceful
    // shutdown before surfacing that error so the command does not leave the peer server running.
    let shutdown = servers.shutdown().await;
    let execution = execution?;
    shutdown?;
    let normalized = normalization::NormalizedSummary::from_execution(&fixture, &execution)?;

    anyhow::bail!(
        "LSP comparison fixture `{}` resolved to {} with {} query cases, \
         methods: {}. Both servers initialized and opened {} source files: {}={}, {}={}. \
         Query execution ran: {}; {}; {}. \
         Normalization ran: {}; {}; {}. \
         Comparison and report rendering are not implemented yet \
         (format: {output_format:?})",
        fixture.kind(),
        fixture.root().display(),
        fixture.query_cases().len(),
        query_summary(fixture.query_cases()),
        opened_files,
        rust_glancer_name,
        format_duration(rust_glancer_initialize),
        rust_analyzer_name,
        format_duration(rust_analyzer_initialize),
        execution.summary_line(),
        execution.server_summary_line(execution::ServerUnderTest::RustGlancer),
        execution.server_summary_line(execution::ServerUnderTest::RustAnalyzer),
        normalized.summary_line(),
        normalized.server_summary_line(execution::ServerUnderTest::RustGlancer),
        normalized.server_summary_line(execution::ServerUnderTest::RustAnalyzer),
    );
}

fn query_summary(query_cases: &[query::QueryCase]) -> String {
    let references = query_cases
        .iter()
        .filter(|query| query.kind().is_references())
        .count();
    let references_with_declaration = query_cases
        .iter()
        .filter_map(|query| query.kind().references_include_declaration())
        .filter(|include_declaration| *include_declaration)
        .count();
    let goto_definition = query_cases
        .iter()
        .filter(|query| query.kind().is_goto_definition())
        .count();
    let hover = query_cases
        .iter()
        .filter(|query| query.kind().is_hover())
        .count();

    format!(
        "references={references} (include_declaration={references_with_declaration}), \
         goto_definition={goto_definition}, hover={hover}",
    )
}

fn format_duration(duration: std::time::Duration) -> String {
    format!("{:.1}ms", duration.as_secs_f64() * 1_000.0)
}
