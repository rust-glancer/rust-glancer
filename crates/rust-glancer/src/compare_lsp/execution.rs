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
    DocumentHighlightParams, DocumentSymbolParams, GotoDefinitionParams, HoverParams,
    InlayHintParams, PartialResultParams, Position, Range, ReferenceContext, ReferenceParams,
    RenameParams, TextDocumentIdentifier, TextDocumentPositionParams, WorkDoneProgressParams,
    WorkspaceSymbolParams,
    request::{GotoImplementationParams, GotoTypeDefinitionParams},
};
use serde_json::Value;

use crate::compare_lsp::{
    fixture::Fixture,
    lsp_client::{RequestOutcome, RpcError},
    query::{QueryCase, QueryKind, QueryTarget, SourcePosition},
    server::{self, StartedServers},
};

const QUERY_TIMEOUT: Duration = Duration::from_secs(120);

/// Raw results from running the fixture query vector against both servers.
#[derive(Debug)]
pub(crate) struct ExecutionSummary {
    results: Vec<QueryExecution>,
}

impl ExecutionSummary {
    pub(crate) fn results(&self) -> &[QueryExecution] {
        &self.results
    }
}

/// Identifies which side of the comparison a stored outcome came from.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ServerUnderTest {
    RustGlancer,
    RustAnalyzer,
}

#[derive(Debug)]
pub(crate) struct QueryExecution {
    label: &'static str,
    kind: QueryKind,
    target: QueryTarget,
    rust_glancer: RawServerOutcome,
    rust_analyzer: RawServerOutcome,
}

impl QueryExecution {
    pub(crate) fn label(&self) -> &'static str {
        self.label
    }

    pub(crate) fn kind(&self) -> QueryKind {
        self.kind
    }

    pub(crate) fn target(&self) -> QueryTarget {
        self.target
    }

    pub(crate) fn outcome(&self, server: ServerUnderTest) -> &RawServerOutcome {
        match server {
            ServerUnderTest::RustGlancer => &self.rust_glancer,
            ServerUnderTest::RustAnalyzer => &self.rust_analyzer,
        }
    }
}

#[derive(Debug)]
pub(crate) struct RawServerOutcome {
    latency: Duration,
    value: RawOutcome,
}

impl RawServerOutcome {
    pub(crate) fn latency(&self) -> Duration {
        self.latency
    }

    pub(crate) fn value(&self) -> &RawOutcome {
        &self.value
    }
}

#[derive(Debug)]
pub(crate) enum RawOutcome {
    Success {
        /// Full response body retained for the later normalization pass.
        raw: Value,
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
    fn status_label(&self) -> &'static str {
        match self {
            Self::Success { .. } => "success",
            Self::Error { .. } => "error",
            Self::Timeout => "timeout",
            Self::TransportFailure { .. } => "transport_failure",
        }
    }

    fn from_request(_kind: QueryKind, outcome: RequestOutcome) -> Self {
        match outcome {
            RequestOutcome::Success(value) => Self::Success { raw: value },
            RequestOutcome::Error(RpcError { code, message, .. }) => Self::Error { code, message },
            RequestOutcome::Timeout => Self::Timeout,
            RequestOutcome::TransportFailure { message } => Self::TransportFailure { message },
        }
    }
}

pub(crate) async fn run(
    fixture: &Fixture,
    servers: &mut StartedServers,
) -> anyhow::Result<ExecutionSummary> {
    let mut results = Vec::new();
    let total_queries = fixture.query_cases().len();

    for (query_index, query_case) in fixture.query_cases().iter().enumerate() {
        // Convert compact fixture entries into typed LSP request payloads at the last boundary.
        // The stored vector stays easy to audit while serde/ls_types still own protocol shape.
        let request = QueryRequest::from_case(fixture, query_case)?;
        tracing::info!(
            query_index = query_index + 1,
            total_queries,
            label = query_case.label(),
            method = request.method,
            "compare-lsp query started"
        );
        let rust_glancer = execute_rust_glancer(servers, query_case.kind(), &request).await;
        tracing::debug!(
            query_index = query_index + 1,
            total_queries,
            label = query_case.label(),
            method = request.method,
            server = "rust-glancer",
            elapsed_ms = rust_glancer.latency().as_millis(),
            status = rust_glancer.value().status_label(),
            "compare-lsp query server completed"
        );
        let rust_analyzer = execute_rust_analyzer(servers, query_case.kind(), &request).await;
        tracing::debug!(
            query_index = query_index + 1,
            total_queries,
            label = query_case.label(),
            method = request.method,
            server = "rust-analyzer",
            elapsed_ms = rust_analyzer.latency().as_millis(),
            status = rust_analyzer.value().status_label(),
            "compare-lsp query server completed"
        );

        results.push(QueryExecution {
            label: query_case.label(),
            kind: query_case.kind(),
            target: query_case.target(),
            rust_glancer,
            rust_analyzer,
        });
    }

    tracing::info!(total_queries, "compare-lsp query execution completed");

    Ok(ExecutionSummary { results })
}

/// Fully materialized LSP request shared by both servers for one query case.
#[derive(Debug)]
struct QueryRequest {
    method: &'static str,
    params: Value,
}

impl QueryRequest {
    fn from_case(fixture: &Fixture, query_case: &QueryCase) -> anyhow::Result<Self> {
        let fixture_root = fixture.root();
        let method = query_case.kind().lsp_method();
        let params = match (query_case.kind(), query_case.target()) {
            (
                QueryKind::References {
                    include_declaration,
                },
                QueryTarget::Position {
                    source_path,
                    position,
                },
            ) => serde_json::to_value(ReferenceParams {
                text_document_position: Self::text_document_position(
                    fixture_root,
                    query_case.label(),
                    source_path,
                    position,
                )?,
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
                context: ReferenceContext {
                    include_declaration,
                },
            }),
            (
                QueryKind::GotoDefinition,
                QueryTarget::Position {
                    source_path,
                    position,
                },
            ) => serde_json::to_value(GotoDefinitionParams {
                text_document_position_params: Self::text_document_position(
                    fixture_root,
                    query_case.label(),
                    source_path,
                    position,
                )?,
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            }),
            (
                QueryKind::TypeDefinition,
                QueryTarget::Position {
                    source_path,
                    position,
                },
            ) => serde_json::to_value(GotoTypeDefinitionParams {
                text_document_position_params: Self::text_document_position(
                    fixture_root,
                    query_case.label(),
                    source_path,
                    position,
                )?,
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            }),
            (
                QueryKind::Implementation,
                QueryTarget::Position {
                    source_path,
                    position,
                },
            ) => serde_json::to_value(GotoImplementationParams {
                text_document_position_params: Self::text_document_position(
                    fixture_root,
                    query_case.label(),
                    source_path,
                    position,
                )?,
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            }),
            (
                QueryKind::PrepareRename,
                QueryTarget::Position {
                    source_path,
                    position,
                },
            ) => serde_json::to_value(Self::text_document_position(
                fixture_root,
                query_case.label(),
                source_path,
                position,
            )?),
            (
                QueryKind::Rename,
                QueryTarget::Rename {
                    source_path,
                    position,
                    new_name,
                },
            ) => serde_json::to_value(RenameParams {
                text_document_position: Self::text_document_position(
                    fixture_root,
                    query_case.label(),
                    source_path,
                    position,
                )?,
                new_name: new_name.to_string(),
                work_done_progress_params: WorkDoneProgressParams::default(),
            }),
            (
                QueryKind::DocumentHighlight,
                QueryTarget::Position {
                    source_path,
                    position,
                },
            ) => serde_json::to_value(DocumentHighlightParams {
                text_document_position_params: Self::text_document_position(
                    fixture_root,
                    query_case.label(),
                    source_path,
                    position,
                )?,
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            }),
            (
                QueryKind::Hover,
                QueryTarget::Position {
                    source_path,
                    position,
                },
            ) => serde_json::to_value(HoverParams {
                text_document_position_params: Self::text_document_position(
                    fixture_root,
                    query_case.label(),
                    source_path,
                    position,
                )?,
                work_done_progress_params: WorkDoneProgressParams::default(),
            }),
            (QueryKind::DocumentSymbol, QueryTarget::File { source_path }) => {
                serde_json::to_value(DocumentSymbolParams {
                    text_document: Self::text_document_identifier(
                        fixture_root,
                        query_case.label(),
                        source_path,
                    )?,
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: PartialResultParams::default(),
                })
            }
            (QueryKind::WorkspaceSymbol, QueryTarget::Workspace { query }) => {
                serde_json::to_value(WorkspaceSymbolParams {
                    partial_result_params: PartialResultParams::default(),
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    query: query.to_string(),
                })
            }
            (QueryKind::InlayHint, QueryTarget::File { source_path }) => {
                serde_json::to_value(InlayHintParams {
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    text_document: Self::text_document_identifier(
                        fixture_root,
                        query_case.label(),
                        source_path,
                    )?,
                    range: Self::full_document_range(fixture, query_case.label(), source_path)?,
                })
            }
            (kind, target) => anyhow::bail!(
                "LSP comparison query `{}` uses incompatible kind {:?} and target {:?}",
                query_case.label(),
                kind,
                target,
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

    fn text_document_position(
        fixture_root: &Path,
        label: &str,
        source_path: &str,
        position: SourcePosition,
    ) -> anyhow::Result<TextDocumentPositionParams> {
        Ok(TextDocumentPositionParams::new(
            Self::text_document_identifier(fixture_root, label, source_path)?,
            position.to_lsp(),
        ))
    }

    fn text_document_identifier(
        fixture_root: &Path,
        label: &str,
        source_path: &str,
    ) -> anyhow::Result<TextDocumentIdentifier> {
        let path = fixture_root.join(source_path);
        let uri = server::file_uri(&path)
            .with_context(|| format!("Creating query URI for `{label}` failed"))?;
        Ok(TextDocumentIdentifier::new(uri))
    }

    fn full_document_range(
        fixture: &Fixture,
        label: &str,
        source_path: &str,
    ) -> anyhow::Result<Range> {
        let source = fixture.editor_source_text(source_path).with_context(|| {
            format!("Reading editor source for LSP comparison query `{label}` failed")
        })?;

        let mut end_line = 0;
        let mut end_character = 0;
        for (line_index, line) in source.lines().enumerate() {
            end_line = line_index as u32;
            end_character = line.encode_utf16().count() as u32;
        }
        if source.ends_with('\n') {
            end_line += 1;
            end_character = 0;
        }

        Ok(Range {
            start: Position::new(0, 0),
            end: Position::new(end_line, end_character),
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
