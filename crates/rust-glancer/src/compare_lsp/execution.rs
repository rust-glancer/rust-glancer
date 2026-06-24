//! Raw query execution for the LSP comparison harness.
//!
//! This layer sends the fixture vector to both live servers and records what came back. It does not
//! normalize locations or compare answers; preserving raw JSON keeps later normalization honest
//! about the response shapes each server actually returns.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use anyhow::Context as _;
use ls_types::{
    GotoDefinitionParams, HoverParams, PartialResultParams, ReferenceContext, ReferenceParams,
    TextDocumentIdentifier, TextDocumentPositionParams, WorkDoneProgressParams, request,
    request::Request as _,
};
use serde_json::Value;

use crate::compare_lsp::{
    fixture::Fixture,
    lsp_client::{RequestOutcome, RpcError},
    query::{QueryCase, QueryKind},
    server::{self, StartedServers},
};

const QUERY_TIMEOUT: Duration = Duration::from_secs(120);

/// Raw results from running the fixture query vector against both servers.
#[derive(Debug)]
pub(crate) struct ExecutionSummary {
    results: Vec<QueryExecution>,
}

impl ExecutionSummary {
    pub(crate) fn summary_line(&self) -> String {
        format!("{} query cases sent to both servers", self.results.len())
    }

    pub(crate) fn server_summary_line(&self, server: ServerUnderTest) -> String {
        let mut success_count = 0;
        let mut error_count = 0;
        let mut timeout_count = 0;
        let mut transport_failure_count = 0;
        let mut first_success_latency = None;
        let mut raw_results = Vec::new();

        for query in &self.results {
            let outcome = query.outcome(server);
            match &outcome.value {
                RawOutcome::Success { .. } => {
                    success_count += 1;
                    first_success_latency.get_or_insert(outcome.latency);
                }
                RawOutcome::Error { .. } => error_count += 1,
                RawOutcome::Timeout => timeout_count += 1,
                RawOutcome::TransportFailure { .. } => transport_failure_count += 1,
            }
            raw_results.push(format!(
                "{}={}",
                query.kind.label(),
                outcome.value.summary()
            ));
        }

        let first_success = first_success_latency
            .map(format_duration)
            .unwrap_or_else(|| "none".to_string());

        format!(
            "{} successes={success_count}, errors={error_count}, timeouts={timeout_count}, \
             transport_failures={transport_failure_count}, first_success={first_success}, raw=[{}]",
            server.label(),
            raw_results.join(", "),
        )
    }
}

/// Identifies which side of the comparison a stored outcome came from.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ServerUnderTest {
    RustGlancer,
    RustAnalyzer,
}

impl ServerUnderTest {
    fn label(self) -> &'static str {
        match self {
            Self::RustGlancer => "rust-glancer",
            Self::RustAnalyzer => "rust-analyzer",
        }
    }
}

#[derive(Debug)]
struct QueryExecution {
    kind: QueryKind,
    rust_glancer: RawServerOutcome,
    rust_analyzer: RawServerOutcome,
}

impl QueryExecution {
    fn outcome(&self, server: ServerUnderTest) -> &RawServerOutcome {
        match server {
            ServerUnderTest::RustGlancer => &self.rust_glancer,
            ServerUnderTest::RustAnalyzer => &self.rust_analyzer,
        }
    }
}

#[derive(Debug)]
struct RawServerOutcome {
    latency: Duration,
    value: RawOutcome,
}

#[derive(Debug)]
enum RawOutcome {
    Success {
        /// Full response body retained for the later normalization pass.
        raw: Value,
        shape: RawSuccessShape,
    },
    Error {
        code: i64,
        message: String,
    },
    Timeout,
    TransportFailure {
        message: String,
    },
}

impl RawOutcome {
    fn from_request(kind: QueryKind, outcome: RequestOutcome) -> Self {
        match outcome {
            RequestOutcome::Success(value) => {
                let shape = RawSuccessShape::from_json(kind, &value);
                Self::Success { raw: value, shape }
            }
            RequestOutcome::Error(RpcError { code, message, .. }) => Self::Error { code, message },
            RequestOutcome::Timeout => Self::Timeout,
            RequestOutcome::TransportFailure { message } => Self::TransportFailure { message },
        }
    }

    fn summary(&self) -> String {
        match self {
            Self::Success { raw, shape } => {
                let _raw_json_available_for_normalization = raw;
                shape.summary()
            }
            Self::Error { code, message } => format!("error({code}: {})", compact(message)),
            Self::Timeout => "timeout".to_string(),
            Self::TransportFailure { message } => {
                format!("transport({})", compact(message))
            }
        }
    }
}

#[derive(Debug)]
enum RawSuccessShape {
    Locations { count: Option<usize> },
    Hover { present: bool },
}

impl RawSuccessShape {
    fn from_json(kind: QueryKind, value: &Value) -> Self {
        match kind {
            QueryKind::References { .. } | QueryKind::GotoDefinition => Self::Locations {
                count: location_count(value),
            },
            QueryKind::Hover => Self::Hover {
                present: !value.is_null(),
            },
        }
    }

    fn summary(&self) -> String {
        match self {
            Self::Locations { count: Some(count) } => format!("{count} locations"),
            Self::Locations { count: None } => "locations=?".to_string(),
            Self::Hover { present: true } => "present".to_string(),
            Self::Hover { present: false } => "absent".to_string(),
        }
    }
}

pub(crate) async fn run(
    fixture: &Fixture,
    servers: &mut StartedServers,
) -> anyhow::Result<ExecutionSummary> {
    let mut results = Vec::new();

    for query_case in fixture.query_cases() {
        // Convert compact fixture entries into typed LSP request payloads at the last boundary.
        // The stored vector stays easy to audit while serde/ls_types still own protocol shape.
        let request = QueryRequest::from_case(fixture.root(), query_case)?;
        let rust_glancer = execute_rust_glancer(servers, query_case.kind(), &request).await;
        let rust_analyzer = execute_rust_analyzer(servers, query_case.kind(), &request).await;

        results.push(QueryExecution {
            kind: query_case.kind(),
            rust_glancer,
            rust_analyzer,
        });
    }

    Ok(ExecutionSummary { results })
}

/// Fully materialized LSP request shared by both servers for one query case.
#[derive(Debug)]
struct QueryRequest {
    method: &'static str,
    params: Value,
}

impl QueryRequest {
    fn from_case(fixture_root: &Path, query_case: &QueryCase) -> anyhow::Result<Self> {
        let path = fixture_root.join(query_case.source_path());
        let uri = server::file_uri(&path)
            .with_context(|| format!("Creating query URI for `{}` failed", query_case.label()))?;
        let position = query_case.position().to_lsp();
        let text_document_position =
            TextDocumentPositionParams::new(TextDocumentIdentifier::new(uri), position);

        let (method, params) = match query_case.kind() {
            QueryKind::References {
                include_declaration,
            } => (
                request::References::METHOD,
                serde_json::to_value(ReferenceParams {
                    text_document_position,
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: PartialResultParams::default(),
                    context: ReferenceContext {
                        include_declaration,
                    },
                }),
            ),
            QueryKind::GotoDefinition => (
                request::GotoDefinition::METHOD,
                serde_json::to_value(GotoDefinitionParams {
                    text_document_position_params: text_document_position,
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: PartialResultParams::default(),
                }),
            ),
            QueryKind::Hover => (
                request::HoverRequest::METHOD,
                serde_json::to_value(HoverParams {
                    text_document_position_params: text_document_position,
                    work_done_progress_params: WorkDoneProgressParams::default(),
                }),
            ),
        };

        Ok(Self {
            method,
            params: params.with_context(|| {
                format!(
                    "Serializing LSP query params for `{}` failed",
                    query_case.label()
                )
            })?,
        })
    }
}

async fn execute_rust_glancer(
    servers: &mut StartedServers,
    kind: QueryKind,
    request: &QueryRequest,
) -> RawServerOutcome {
    let started_at = Instant::now();
    let outcome = servers
        .request_rust_glancer(request.method, request.params.clone(), QUERY_TIMEOUT)
        .await;
    RawServerOutcome {
        latency: started_at.elapsed(),
        value: RawOutcome::from_request(kind, outcome),
    }
}

async fn execute_rust_analyzer(
    servers: &mut StartedServers,
    kind: QueryKind,
    request: &QueryRequest,
) -> RawServerOutcome {
    let started_at = Instant::now();
    let outcome = servers
        .request_rust_analyzer(request.method, request.params.clone(), QUERY_TIMEOUT)
        .await;
    RawServerOutcome {
        latency: started_at.elapsed(),
        value: RawOutcome::from_request(kind, outcome),
    }
}

fn location_count(value: &Value) -> Option<usize> {
    if value.is_null() {
        return Some(0);
    }
    if let Some(locations) = value.as_array() {
        return Some(locations.len());
    }
    if value.is_object() {
        return Some(1);
    }

    None
}

fn compact(message: &str) -> String {
    let mut message = message.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX_LEN: usize = 80;
    if message.len() > MAX_LEN {
        message.truncate(MAX_LEN);
        message.push_str("...");
    }
    message
}

fn format_duration(duration: Duration) -> String {
    format!("{:.1}ms", duration.as_secs_f64() * 1_000.0)
}

trait QueryKindLabel {
    fn label(self) -> &'static str;
}

impl QueryKindLabel for QueryKind {
    fn label(self) -> &'static str {
        match self {
            Self::References { .. } => "references",
            Self::GotoDefinition => "goto_definition",
            Self::Hover => "hover",
        }
    }
}
