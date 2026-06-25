//! Paired process lifecycle for the compared LSP servers.
//!
//! This module owns the shared setup for both sides of the comparison: spawn each server, send
//! initialize/didOpen, forward query requests, and shut both processes down. Query construction and
//! raw result collection live outside this module so process handling stays independent of the
//! request family being compared.

mod command;
mod process;
mod stderr;
mod uri;

use std::time::Duration;

use serde_json::Value;

use crate::compare_lsp::{fixture::Fixture, lsp_client::RequestOutcome, query::QueryCase};

use self::{command::ServerKind, process::RunningServer};

pub(crate) use self::uri::file_uri;

/// Two initialized servers that have opened the same fixture files.
#[derive(Debug)]
pub(crate) struct StartedServers {
    rust_glancer_server: RunningServer,
    rust_analyzer_server: RunningServer,
    rust_glancer_readiness: ServerReadiness,
    rust_analyzer_readiness: ServerReadiness,
    opened_files: usize,
}

impl StartedServers {
    /// Spawn both servers and prepare them to answer the fixture query vector.
    pub(crate) async fn start(fixture: &Fixture) -> anyhow::Result<Self> {
        let source_paths = unique_source_paths(fixture.query_cases());
        let mut rust_glancer_server = RunningServer::spawn(ServerKind::RustGlancer).await?;
        let mut rust_analyzer_server = RunningServer::spawn(ServerKind::RustAnalyzer).await?;

        let rust_glancer_readiness = rust_glancer_server
            .initialize_fixture(fixture.root(), &source_paths)
            .await?;
        let rust_analyzer_readiness = rust_analyzer_server
            .initialize_fixture(fixture.root(), &source_paths)
            .await?;

        Ok(Self {
            rust_glancer_server,
            rust_analyzer_server,
            rust_glancer_readiness,
            rust_analyzer_readiness,
            opened_files: source_paths.len(),
        })
    }

    pub(crate) fn rust_glancer_readiness(&self) -> &ServerReadiness {
        &self.rust_glancer_readiness
    }

    pub(crate) fn rust_analyzer_readiness(&self) -> &ServerReadiness {
        &self.rust_analyzer_readiness
    }

    pub(crate) fn rust_glancer_command_label(&self) -> &str {
        self.rust_glancer_server.command_label()
    }

    pub(crate) fn rust_analyzer_command_label(&self) -> &str {
        self.rust_analyzer_server.command_label()
    }

    pub(crate) async fn request_rust_glancer(
        &mut self,
        method: &'static str,
        params: Value,
        timeout: Duration,
    ) -> RequestOutcome {
        self.rust_glancer_server
            .request(method, params, timeout)
            .await
    }

    pub(crate) async fn request_rust_analyzer(
        &mut self,
        method: &'static str,
        params: Value,
        timeout: Duration,
    ) -> RequestOutcome {
        self.rust_analyzer_server
            .request(method, params, timeout)
            .await
    }

    pub(crate) fn opened_files(&self) -> usize {
        self.opened_files
    }

    /// Ask both servers to shut down, even if one side reports an error first.
    pub(crate) async fn shutdown(self) -> anyhow::Result<()> {
        let (rust_glancer_shutdown, rust_analyzer_shutdown) = futures::future::join(
            self.rust_glancer_server.shutdown(),
            self.rust_analyzer_server.shutdown(),
        )
        .await;
        match (rust_glancer_shutdown, rust_analyzer_shutdown) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(rust_glancer_error), Err(rust_analyzer_error)) => {
                anyhow::bail!(
                    "both LSP servers failed during shutdown\n\
                     rust-glancer: {rust_glancer_error}\n\
                     rust-analyzer: {rust_analyzer_error}",
                );
            }
        }
    }
}

/// Initialization facts reported with the comparison run.
#[derive(Debug)]
pub(crate) struct ServerReadiness {
    name: &'static str,
    initialize_latency: Duration,
    ready_latency: Duration,
}

impl ServerReadiness {
    pub(super) fn new(
        name: &'static str,
        initialize_latency: Duration,
        ready_latency: Duration,
    ) -> Self {
        Self {
            name,
            initialize_latency,
            ready_latency,
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        self.name
    }

    pub(crate) fn initialize_latency(&self) -> Duration {
        self.initialize_latency
    }

    pub(crate) fn ready_latency(&self) -> Duration {
        self.ready_latency
    }
}

fn unique_source_paths(query_cases: &[QueryCase]) -> Vec<&'static str> {
    let mut source_paths = Vec::new();
    for query_case in query_cases {
        let Some(source_path) = query_case.source_path() else {
            continue;
        };
        if !source_paths.contains(&source_path) {
            source_paths.push(source_path);
        }
    }
    source_paths
}
