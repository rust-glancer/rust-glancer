//! Raw query execution for the LSP comparison harness.
//!
//! This layer sends the fixture vector to both live servers and records what came back. It does not
//! normalize locations or compare answers; preserving raw JSON keeps later normalization honest
//! about the response shapes each server actually returns.

use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::Context as _;
use gen_lsp_types::{
    DefinitionParams, DocumentHighlightParams, DocumentSymbolParams, HoverParams,
    ImplementationParams, InlayHintParams, PartialResultParams, Position, Range, ReferenceContext,
    ReferenceParams, RenameParams, TextDocumentIdentifier, TextDocumentPositionParams,
    TypeDefinitionParams, WorkDoneProgressParams, WorkspaceSymbolParams,
};
use serde::Serialize;
use serde_json::Value;

use crate::{
    fixture::Fixture,
    lsp_client::{RequestOutcome, RpcError},
    query::{QueryCase, QueryKind, QueryTarget, SourcePosition},
    server::{self, RunningServer, ServerKind},
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

    /// Save raw LSP responses as agent-debug artifacts to simplify debugging and exact-location comparisons.
    pub(crate) fn write_raw_results(&self) -> anyhow::Result<()> {
        let Some(directory) = env::var_os("RUST_GLANCER_AGENT_DEBUG_OUTPUT_DIR") else {
            return Ok(());
        };
        let results: Vec<_> = self
            .results
            .iter()
            .map(|query| {
                serde_json::json!({
                    "label": query.label,
                    "method": query.kind.lsp_method(),
                    "rust_glancer": {
                        "elapsed_ms": query.rust_glancer.latency.as_secs_f64() * 1000.0,
                        "outcome": query.rust_glancer.value,
                    },
                    "rust_analyzer": {
                        "elapsed_ms": query.rust_analyzer.latency.as_secs_f64() * 1000.0,
                        "outcome": query.rust_analyzer.value,
                    },
                })
            })
            .collect();
        let path = PathBuf::from(directory).join("lsp-comparison-raw.json");
        fs::write(path, serde_json::to_vec_pretty(&results)?)
            .context("write raw LSP comparison results")
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

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
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
    fn from_request(outcome: RequestOutcome) -> Self {
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
) -> anyhow::Result<(ExecutionSummary, usize, Vec<super::report::ServerReport>)> {
    let paths = fixture
        .query_cases()
        .iter()
        .filter_map(QueryCase::source_path)
        .collect::<rg_std::UniqueVec<_>>();
    let requests = fixture
        .query_cases()
        .iter()
        .map(|case| QueryRequest::from_case(fixture, case))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let (glancer, glancer_report) = run_sweep(
        fixture,
        paths.as_slice(),
        &requests,
        ServerKind::RustGlancer,
    )
    .await?;
    let (analyzer, analyzer_report) = run_sweep(
        fixture,
        paths.as_slice(),
        &requests,
        ServerKind::RustAnalyzer,
    )
    .await?;
    let results = fixture
        .query_cases()
        .iter()
        .zip(glancer.into_iter().zip(analyzer))
        .map(|(case, (rust_glancer, rust_analyzer))| QueryExecution {
            label: case.label(),
            kind: case.kind(),
            target: case.target(),
            rust_glancer,
            rust_analyzer,
        })
        .collect();
    Ok((
        ExecutionSummary { results },
        paths.len(),
        vec![glancer_report, analyzer_report],
    ))
}

async fn run_sweep(
    fixture: &Fixture,
    paths: &[&str],
    requests: &[QueryRequest],
    kind: ServerKind,
) -> anyhow::Result<(Vec<RawServerOutcome>, super::report::ServerReport)> {
    let mut server = RunningServer::spawn(kind, None, None)?;
    let ready = server.initialize_fixture(fixture.root(), paths).await?;
    let report = super::report::ServerReport::capture(
        ready.name(),
        server.command_label(),
        ready.initialize_latency(),
        ready.ready_latency(),
    );
    if fixture.uses_dirty_editor_text() {
        for path in paths {
            server
                .change_source_file(fixture.root(), path, fixture.editor_source_text(path)?)
                .await?;
        }
    }
    let mut results = Vec::with_capacity(requests.len());
    for (case, request) in fixture.query_cases().iter().zip(requests) {
        tracing::debug!(
            server = kind.display_name(),
            query = case.label(),
            "compare-lsp query started"
        );
        let started_at = Instant::now();
        let (received_at, outcome) = server
            .observe_request(request.method, request.params.clone(), QUERY_TIMEOUT)
            .await;
        results.push(RawServerOutcome {
            latency: received_at.duration_since(started_at),
            value: RawOutcome::from_request(outcome),
        });
    }
    server.shutdown().await?;
    Ok((results, report))
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
                text_document_position_params: Self::text_document_position(
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
            ) => serde_json::to_value(DefinitionParams {
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
            ) => serde_json::to_value(TypeDefinitionParams {
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
            ) => serde_json::to_value(ImplementationParams {
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
                text_document_position_params: Self::text_document_position(
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
