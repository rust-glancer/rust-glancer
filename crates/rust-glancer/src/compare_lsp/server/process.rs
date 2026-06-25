//! Lifecycle for one spawned LSP server process.
//!
//! `RunningServer` owns the OS process, its transport, and the captured stderr snippet used in
//! error messages. The paired-server module decides which two servers run; this file keeps the
//! single-process protocol sequence readable.

use std::{
    fs,
    path::Path,
    process::{ExitStatus, Stdio},
    time::{Duration, Instant},
};

use anyhow::Context as _;
use ls_types::{
    ClientCapabilities, DidOpenTextDocumentParams, DocumentSymbolClientCapabilities,
    InitializeParams, InitializedParams, InlayHintClientCapabilities,
    TextDocumentClientCapabilities, TextDocumentItem, WindowClientCapabilities,
    WorkDoneProgressParams, WorkspaceClientCapabilities, WorkspaceFolder, notification,
    notification::Notification as _, request, request::Request as _,
};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::process::{Child, Command};

use crate::compare_lsp::lsp_client::{RequestOutcome, ServerNotification, TowerLspTransport};

use super::{ServerReadiness, command::ServerKind, stderr::StderrCapture, uri::file_uri};

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(120);
const READY_TIMEOUT: Duration = Duration::from_secs(120);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const PROCESS_EXIT_TIMEOUT: Duration = Duration::from_secs(120);
const RUST_GLANCER_READY_METHOD: &str = "rust-glancer/activeWorkspaceChanged";
const RUST_ANALYZER_READY_METHOD: &str = "experimental/serverStatus";

/// One live LSP server with the client-side transport needed to drive it.
#[derive(Debug)]
pub(super) struct RunningServer {
    kind: ServerKind,
    command_label: String,
    child: Child,
    client: TowerLspTransport,
    stderr: StderrCapture,
    exited: bool,
}

impl RunningServer {
    /// Spawn the executable for one side of the comparison and attach stdio transport.
    pub(super) async fn spawn(kind: ServerKind) -> anyhow::Result<Self> {
        let command_spec = kind.command_spec()?;
        let command_label = command_spec.label();
        let mut command = Command::new(&command_spec.executable);
        command
            .args(&command_spec.arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().with_context(|| {
            format!(
                "Spawning {} LSP server with `{command_label}` failed",
                kind.display_name()
            )
        })?;
        let stdin = child.stdin.take().with_context(|| {
            format!(
                "Opening stdin for {} LSP server failed",
                kind.display_name()
            )
        })?;
        let stdout = child.stdout.take().with_context(|| {
            format!(
                "Opening stdout for {} LSP server failed",
                kind.display_name()
            )
        })?;
        let stderr = child.stderr.take().with_context(|| {
            format!(
                "Opening stderr for {} LSP server failed",
                kind.display_name()
            )
        })?;
        let client = TowerLspTransport::spawn(stdout, stdin);
        let stderr = StderrCapture::spawn(stderr);

        Ok(Self {
            kind,
            command_label,
            child,
            client,
            stderr,
            exited: false,
        })
    }

    pub(super) async fn initialize_fixture(
        &mut self,
        fixture_root: &Path,
        source_paths: &[&'static str],
    ) -> anyhow::Result<ServerReadiness> {
        let initialize_params = self.initialize_params(fixture_root)?;
        let started_at = Instant::now();
        let initialize = self
            .client
            .request(
                request::Initialize::METHOD,
                initialize_params,
                INITIALIZE_TIMEOUT,
            )
            .await;
        self.expect_success(request::Initialize::METHOD, initialize)?;
        let initialize_latency = started_at.elapsed();

        // After `initialized`, both servers should see the same in-memory document set. This keeps
        // the later query requests about server behavior rather than file-watcher timing.
        self.client
            .notify(
                notification::Initialized::METHOD,
                lsp_params(InitializedParams {}, "initialized notification")?,
            )
            .await
            .with_context(|| {
                format!(
                    "Sending initialized notification to {} failed",
                    self.kind.display_name()
                )
            })?;

        for source_path in source_paths {
            self.open_source_file(fixture_root, source_path).await?;
        }
        let ready_started_at = Instant::now();
        self.wait_until_ready().await?;
        let ready_latency = ready_started_at.elapsed();

        Ok(ServerReadiness::new(
            self.kind.display_name(),
            initialize_latency,
            ready_latency,
        ))
    }

    /// Run the LSP shutdown handshake, then wait until the OS process exits.
    pub(super) async fn shutdown(mut self) -> anyhow::Result<()> {
        let shutdown = self
            .client
            .request(request::Shutdown::METHOD, Value::Null, SHUTDOWN_TIMEOUT)
            .await;
        self.expect_success(request::Shutdown::METHOD, shutdown)?;
        self.client
            .notify(
                notification::Exit::METHOD,
                lsp_params((), "exit notification")?,
            )
            .await
            .with_context(|| {
                format!(
                    "Sending exit notification to {} failed",
                    self.kind.display_name()
                )
            })?;
        let status = self.wait_for_exit(PROCESS_EXIT_TIMEOUT).await?;
        if !status.success() {
            anyhow::bail!(
                "{} LSP server exited with status {status} after shutdown{}",
                self.kind.display_name(),
                self.stderr_note(),
            );
        }

        Ok(())
    }

    pub(super) async fn request(
        &mut self,
        method: &'static str,
        params: Value,
        timeout: Duration,
    ) -> RequestOutcome {
        self.client.request(method, params, timeout).await
    }

    pub(super) fn command_label(&self) -> &str {
        &self.command_label
    }

    /// Build the client capabilities and workspace identity shared by both compared servers.
    fn initialize_params(&self, fixture_root: &Path) -> anyhow::Result<Value> {
        let root_uri = file_uri(fixture_root)?;
        let root_name = fixture_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("fixture");

        let capabilities = ClientCapabilities {
            window: Some(WindowClientCapabilities {
                work_done_progress: Some(true),
                ..WindowClientCapabilities::default()
            }),
            workspace: Some(WorkspaceClientCapabilities {
                configuration: Some(true),
                workspace_folders: Some(true),
                ..WorkspaceClientCapabilities::default()
            }),
            text_document: Some(TextDocumentClientCapabilities {
                document_symbol: Some(DocumentSymbolClientCapabilities {
                    hierarchical_document_symbol_support: Some(true),
                    ..DocumentSymbolClientCapabilities::default()
                }),
                inlay_hint: Some(InlayHintClientCapabilities::default()),
                ..TextDocumentClientCapabilities::default()
            }),
            experimental: Some(json!({
                "serverStatusNotification": true,
            })),
            ..ClientCapabilities::default()
        };

        #[allow(deprecated)]
        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_path: None,
            root_uri: Some(root_uri.clone()),
            initialization_options: Some(self.kind.initialization_options()),
            capabilities,
            trace: None,
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: root_uri,
                name: root_name.to_string(),
            }]),
            client_info: None,
            locale: None,
            work_done_progress_params: WorkDoneProgressParams::default(),
        };

        lsp_params(params, "initialize params")
    }

    async fn open_source_file(
        &mut self,
        fixture_root: &Path,
        source_path: &str,
    ) -> anyhow::Result<()> {
        let path = fixture_root.join(source_path);
        let text = fs::read_to_string(&path).with_context(|| {
            format!(
                "Reading fixture source file {} for didOpen failed",
                path.display()
            )
        })?;
        let uri = file_uri(&path)?;

        // The fixture file is opened by value, not discovered through the server's file watching.
        // That makes the query vector deterministic for custom fixture paths as well as defaults.
        self.client
            .notify(
                notification::DidOpenTextDocument::METHOD,
                lsp_params(
                    DidOpenTextDocumentParams {
                        text_document: TextDocumentItem::new(uri, "rust".to_string(), 1, text),
                    },
                    "didOpen params",
                )?,
            )
            .await
            .with_context(|| {
                format!(
                    "Sending didOpen for {} to {} failed",
                    path.display(),
                    self.kind.display_name()
                )
            })
    }

    async fn wait_until_ready(&mut self) -> anyhow::Result<()> {
        tokio::time::timeout(READY_TIMEOUT, async {
            loop {
                let notification = self.client.next_notification().await?;
                match self.readiness_notification(&notification) {
                    ReadinessNotification::Ready => return Ok(()),
                    ReadinessNotification::Failed(message) => anyhow::bail!(
                        "{} reported readiness failure: {message}",
                        self.kind.display_name(),
                    ),
                    ReadinessNotification::Ignore => {}
                }
            }
        })
        .await
        .with_context(|| {
            format!(
                "Waiting for {} readiness notification timed out",
                self.kind.display_name()
            )
        })?
    }

    fn readiness_notification(&self, notification: &ServerNotification) -> ReadinessNotification {
        match self.kind {
            ServerKind::RustGlancer => rust_glancer_readiness(notification),
            ServerKind::RustAnalyzer => rust_analyzer_readiness(notification),
        }
    }

    /// Add protocol context and the captured stderr tail to request failures.
    fn expect_success(
        &self,
        method: &'static str,
        outcome: RequestOutcome,
    ) -> anyhow::Result<Value> {
        match outcome {
            RequestOutcome::Success(value) => Ok(value),
            RequestOutcome::Error(error) => {
                let data = error
                    .data
                    .map(|data| format!(" data={data}"))
                    .unwrap_or_default();
                anyhow::bail!(
                    "{} LSP request `{method}` failed with code {}: {}{}{}",
                    self.kind.display_name(),
                    error.code,
                    error.message,
                    data,
                    self.stderr_note(),
                )
            }
            RequestOutcome::Timeout => {
                anyhow::bail!(
                    "{} LSP request `{method}` timed out{}",
                    self.kind.display_name(),
                    self.stderr_note(),
                )
            }
            RequestOutcome::TransportFailure { message } => {
                anyhow::bail!(
                    "{} LSP request `{method}` failed at the transport layer: {message}{}",
                    self.kind.display_name(),
                    self.stderr_note(),
                )
            }
        }
    }

    /// Prefer graceful exit after shutdown, but reap the child if it does not leave on its own.
    async fn wait_for_exit(&mut self, timeout: Duration) -> anyhow::Result<ExitStatus> {
        match tokio::time::timeout(timeout, self.child.wait()).await {
            Ok(status) => {
                let status = status.with_context(|| {
                    format!(
                        "Waiting for {} LSP process failed",
                        self.kind.display_name()
                    )
                })?;
                self.exited = true;
                self.stderr.join().await;
                Ok(status)
            }
            Err(_elapsed) => {
                let _ = self.child.start_kill();
                let status = self.child.wait().await.with_context(|| {
                    format!(
                        "Reaping timed out {} LSP process failed",
                        self.kind.display_name()
                    )
                })?;
                self.exited = true;
                self.stderr.join().await;
                anyhow::bail!(
                    "{} LSP server did not exit within {:?} after shutdown; killed with status {status}{}",
                    self.kind.display_name(),
                    timeout,
                    self.stderr_note(),
                );
            }
        }
    }

    fn stderr_note(&self) -> String {
        let snippet = self.stderr.snippet();
        if snippet.is_empty() {
            String::new()
        } else {
            format!("\nstderr from `{}`:\n{snippet}", self.command_label)
        }
    }
}

fn lsp_params(params: impl Serialize, description: &'static str) -> anyhow::Result<Value> {
    serde_json::to_value(params).with_context(|| format!("Serializing {description} failed"))
}

enum ReadinessNotification {
    Ready,
    Failed(String),
    Ignore,
}

fn rust_glancer_readiness(notification: &ServerNotification) -> ReadinessNotification {
    if notification.method() != RUST_GLANCER_READY_METHOD {
        return ReadinessNotification::Ignore;
    }

    let Some(params) = notification.params() else {
        return ReadinessNotification::Ignore;
    };
    match params.get("state").and_then(Value::as_str) {
        Some("ready") => ReadinessNotification::Ready,
        Some("failed") => ReadinessNotification::Failed(
            params
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("workspace failed")
                .to_string(),
        ),
        Some(_) | None => ReadinessNotification::Ignore,
    }
}

fn rust_analyzer_readiness(notification: &ServerNotification) -> ReadinessNotification {
    if notification.method() != RUST_ANALYZER_READY_METHOD {
        return ReadinessNotification::Ignore;
    }

    let Some(params) = notification.params() else {
        return ReadinessNotification::Ignore;
    };
    if params.get("health").and_then(Value::as_str) == Some("error") {
        return ReadinessNotification::Failed(
            params
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("server reported error status")
                .to_string(),
        );
    }
    if params.get("quiescent").and_then(Value::as_bool) == Some(true) {
        return ReadinessNotification::Ready;
    }

    ReadinessNotification::Ignore
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        if self.exited {
            return;
        }

        // Drop cannot await process exit, so the best fallback is to avoid leaving the child
        // running if an earlier error short-circuits the normal shutdown path.
        match self.child.try_wait() {
            Ok(Some(_status)) => {
                self.exited = true;
            }
            Ok(None) => {
                let _ = self.child.start_kill();
                self.exited = true;
            }
            Err(_error) => {}
        }
    }
}
