//! Lifecycle for one spawned LSP server process.
//!
//! `RunningServer` owns the OS process, its transport, and the captured stderr snippet used in
//! error messages.

use std::{
    fs,
    path::Path,
    process::{ExitStatus, Stdio},
    time::{Duration, Instant},
};

use anyhow::Context as _;
use gen_lsp_types::{
    ClientCapabilities, ClientCompletionItemOptions, ClientCompletionItemResolveOptions,
    ClientSemanticTokensRequestOptions, CompletionClientCapabilities, DidChangeTextDocumentParams,
    DidOpenTextDocumentParams, DocumentSymbolClientCapabilities, HoverClientCapabilities,
    InitializeParams, InitializedParams, InlayHintClientCapabilities, LanguageKind, MarkupKind,
    Notification as _, RenameClientCapabilities, Request as _, SemanticTokensClientCapabilities,
    TextDocumentClientCapabilities, TextDocumentContentChangeWholeDocument, TextDocumentIdentifier,
    TextDocumentItem, TokenFormat, VersionedTextDocumentIdentifier, WindowClientCapabilities,
    WorkDoneProgressParams, WorkspaceClientCapabilities, WorkspaceEditClientCapabilities,
    WorkspaceFolder, WorkspaceFoldersInitializeParams,
};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::process::{Child, Command};

use super::{ServerReadiness, command::ServerKind, stderr::StderrCapture, uri::file_uri};
use crate::lsp_client::{RequestOutcome, ServerNotification, TowerLspTransport};

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(120);
const READY_TIMEOUT: Duration = Duration::from_secs(120);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const PROCESS_EXIT_TIMEOUT: Duration = Duration::from_secs(120);
const RUST_GLANCER_READY_METHOD: &str = "rust-glancer/activeWorkspaceChanged";
const SERVER_STATUS_METHOD: &str = "experimental/serverStatus";

/// One live LSP server with the client-side transport needed to drive it.
#[derive(Debug)]
pub(crate) struct RunningServer {
    kind: ServerKind,
    command_label: String,
    spawned_at: Instant,
    child: Child,
    client: TowerLspTransport,
    stderr: StderrCapture,
    exited: bool,
}

impl RunningServer {
    /// Spawn the executable for one side of the comparison and attach stdio transport.
    pub(crate) fn spawn(
        kind: ServerKind,
        executable: Option<&Path>,
        workspace: Option<(&Path, &Path, &str)>,
    ) -> anyhow::Result<Self> {
        let command_spec = kind.command_spec(executable)?;
        let command_label = command_spec.label();
        let mut command = Command::new(&command_spec.executable);
        command
            .args(&command_spec.arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some((root, cargo_output, toolchain)) = workspace {
            command
                .current_dir(root)
                .env("CARGO_TARGET_DIR", cargo_output)
                .env("CARGO_BUILD_BUILD_DIR", cargo_output)
                .env("CARGO_NET_OFFLINE", "true")
                .env("RUSTUP_TOOLCHAIN", toolchain);
            if matches!(kind, ServerKind::RustAnalyzer) {
                // The benchmark's rust-analyzer supports this override on all platforms.
                // Keep the user's rust-analyzer.toml out of the benchmark configuration.
                let user_config = cargo_output.with_file_name("rust-analyzer-config");
                fs::create_dir_all(&user_config)
                    .context("create isolated rust-analyzer configuration directory")?;
                command.env("__TEST_RA_USER_CONFIG_DIR", user_config);
            }
        }
        let spawned_at = Instant::now();
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
            spawned_at,
            child,
            client,
            stderr,
            exited: false,
        })
    }

    pub(crate) async fn initialize_fixture(
        &mut self,
        fixture_root: &Path,
        source_paths: &[&str],
    ) -> anyhow::Result<ServerReadiness> {
        let initialize_latency = self
            .initialize(fixture_root, self.kind.initialization_options(false, false))
            .await?;
        for source_path in source_paths {
            self.open_source_file(fixture_root, source_path).await?;
        }
        let ready_started_at = Instant::now();
        self.wait_until_ready().await?;
        Ok(ServerReadiness::new(
            self.kind.display_name(),
            initialize_latency,
            ready_started_at.elapsed(),
        ))
    }

    pub(crate) async fn initialize(
        &mut self,
        root: &Path,
        options: Value,
    ) -> anyhow::Result<Duration> {
        let params = self.initialize_params(root, options)?;
        let started_at = Instant::now();
        let outcome = self
            .request(
                gen_lsp_types::InitializeRequest::METHOD.as_str(),
                params,
                INITIALIZE_TIMEOUT,
            )
            .await;
        self.expect_success(gen_lsp_types::InitializeRequest::METHOD.as_str(), outcome)?;
        let elapsed = started_at.elapsed();
        self.notify(
            gen_lsp_types::InitializedNotification::METHOD.as_str(),
            Self::lsp_params(InitializedParams {}, "initialized notification")?,
        )
        .await?;
        Ok(elapsed)
    }

    pub(crate) fn spawned_at(&self) -> Instant {
        self.spawned_at
    }

    /// Run the LSP shutdown handshake, then wait until the OS process exits.
    pub(crate) async fn shutdown(mut self) -> anyhow::Result<()> {
        let result = self.graceful_shutdown().await;
        if result.is_err() && !self.exited {
            // Reap the direct child even when the protocol handshake fails. The benchmark
            // stops after a cleanup failure because descendants may still own resources.
            self.exited = tokio::time::timeout(SHUTDOWN_TIMEOUT, self.child.kill())
                .await
                .is_ok_and(|result| result.is_ok());
        }
        result
    }

    async fn graceful_shutdown(&mut self) -> anyhow::Result<()> {
        let shutdown = self
            .request(
                gen_lsp_types::ShutdownRequest::METHOD.as_str(),
                Value::Null,
                SHUTDOWN_TIMEOUT,
            )
            .await;
        self.expect_success(gen_lsp_types::ShutdownRequest::METHOD.as_str(), shutdown)?;
        self.client
            .notify(
                gen_lsp_types::ExitNotification::METHOD.as_str(),
                Self::lsp_params((), "exit notification")?,
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

    pub(crate) async fn request(
        &self,
        method: &'static str,
        params: Value,
        timeout: Duration,
    ) -> RequestOutcome {
        self.observe_request(method, params, timeout).await.1
    }

    pub(crate) async fn observe_request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> (Instant, RequestOutcome) {
        self.client.request(method, params, timeout).await
    }

    pub(crate) async fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
        self.client.notify(method, params).await
    }

    pub(crate) fn command_label(&self) -> &str {
        &self.command_label
    }

    pub(crate) fn client_capabilities() -> ClientCapabilities {
        ClientCapabilities {
            window: Some(WindowClientCapabilities {
                work_done_progress: Some(true),
                ..WindowClientCapabilities::default()
            }),
            workspace: Some(WorkspaceClientCapabilities {
                configuration: Some(true),
                workspace_folders: Some(true),
                workspace_edit: Some(WorkspaceEditClientCapabilities {
                    document_changes: Some(true),
                    ..WorkspaceEditClientCapabilities::default()
                }),
                ..WorkspaceClientCapabilities::default()
            }),
            text_document: Some(TextDocumentClientCapabilities {
                document_symbol: Some(DocumentSymbolClientCapabilities {
                    hierarchical_document_symbol_support: Some(true),
                    ..DocumentSymbolClientCapabilities::default()
                }),
                inlay_hint: Some(InlayHintClientCapabilities::default()),
                hover: Some(HoverClientCapabilities {
                    content_format: Some(vec![MarkupKind::Markdown, MarkupKind::PlainText]),
                    ..HoverClientCapabilities::default()
                }),
                completion: Some(CompletionClientCapabilities {
                    completion_item: Some(ClientCompletionItemOptions {
                        resolve_support: Some(ClientCompletionItemResolveOptions::new(vec![
                            "additionalTextEdits".to_owned(),
                        ])),
                        ..ClientCompletionItemOptions::default()
                    }),
                    ..CompletionClientCapabilities::default()
                }),
                semantic_tokens: Some(SemanticTokensClientCapabilities {
                    requests: ClientSemanticTokensRequestOptions {
                        range: Some(true.into()),
                        ..ClientSemanticTokensRequestOptions::default()
                    },
                    token_types: rg_lsp_proto::SEMANTIC_TOKEN_TYPES
                        .iter()
                        .map(|kind| kind.as_str().to_owned())
                        .collect(),
                    token_modifiers: rg_lsp_proto::SEMANTIC_TOKEN_MODIFIERS
                        .iter()
                        .map(|modifier| modifier.as_str().to_owned())
                        .collect(),
                    formats: vec![TokenFormat::Relative],
                    // The editor supplies syntax highlighting. Without this capability,
                    // rust-analyzer also sends tokens for plain syntax such as punctuation.
                    augments_syntax_tokens: Some(true),
                    ..SemanticTokensClientCapabilities::default()
                }),
                rename: Some(RenameClientCapabilities {
                    prepare_support: Some(true),
                    ..RenameClientCapabilities::default()
                }),
                ..TextDocumentClientCapabilities::default()
            }),
            experimental: Some(json!({
                "serverStatusNotification": true,
            })),
            ..ClientCapabilities::default()
        }
    }

    fn initialize_params(&self, fixture_root: &Path, options: Value) -> anyhow::Result<Value> {
        let root_uri = file_uri(fixture_root)?;
        let root_name = fixture_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("fixture");

        #[allow(deprecated)]
        let params = InitializeParams {
            process_id: Some(
                i32::try_from(std::process::id())
                    .context("process ID exceeds LSP integer range")?,
            ),
            root_path: None,
            root_uri: Some(root_uri.clone()),
            initialization_options: Some(options),
            capabilities: Self::client_capabilities(),
            trace: None,
            workspace_folders_initialize_params: WorkspaceFoldersInitializeParams {
                workspace_folders: Some(
                    vec![WorkspaceFolder {
                        uri: root_uri,
                        name: root_name.to_string(),
                    }]
                    .into(),
                ),
            },
            client_info: None,
            locale: None,
            work_done_progress_params: WorkDoneProgressParams::default(),
        };

        Self::lsp_params(params, "initialize params")
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
                gen_lsp_types::DidOpenTextDocumentNotification::METHOD.as_str(),
                Self::lsp_params(
                    DidOpenTextDocumentParams {
                        text_document: TextDocumentItem::new(uri, LanguageKind::Rust, 1, text),
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

    /// Replace one open document with the stable unsaved text used by the dirty fixture.
    pub(crate) async fn change_source_file(
        &mut self,
        fixture_root: &Path,
        source_path: &str,
        text: String,
    ) -> anyhow::Result<()> {
        let path = fixture_root.join(source_path);
        let uri = file_uri(&path)?;
        self.client
            .notify(
                gen_lsp_types::DidChangeTextDocumentNotification::METHOD.as_str(),
                Self::lsp_params(
                    DidChangeTextDocumentParams {
                        text_document: VersionedTextDocumentIdentifier {
                            text_document_identifier: TextDocumentIdentifier { uri },
                            version: 2,
                        },
                        content_changes: vec![
                            TextDocumentContentChangeWholeDocument { text }.into(),
                        ],
                    },
                    "didChange params",
                )?,
            )
            .await
            .with_context(|| {
                format!(
                    "Sending dirty didChange for {} to {} failed",
                    path.display(),
                    self.kind.display_name(),
                )
            })
    }

    pub(crate) async fn wait_until_ready(&mut self) -> anyhow::Result<Instant> {
        tokio::time::timeout(READY_TIMEOUT, async {
            loop {
                let notification = self.client.next_notification().await.with_context(|| {
                    format!(
                        "Waiting for {} readiness notification failed{}",
                        self.kind.display_name(),
                        self.stderr_note(),
                    )
                })?;
                match self.readiness_notification(&notification) {
                    ReadinessNotification::Ready => return Ok(notification.received_at()),
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
                "Waiting for {} readiness notification timed out{}",
                self.kind.display_name(),
                self.stderr_note(),
            )
        })?
    }

    fn readiness_notification(&self, notification: &ServerNotification) -> ReadinessNotification {
        match self.kind {
            ServerKind::RustGlancer => Self::rust_glancer_readiness(notification),
            ServerKind::RustAnalyzer => Self::rust_analyzer_readiness(notification),
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
                self.stderr.join(SHUTDOWN_TIMEOUT).await?;
                Ok(status)
            }
            Err(_elapsed) => anyhow::bail!(
                "{} LSP server did not exit within {timeout:?} after shutdown{}",
                self.kind.display_name(),
                self.stderr_note(),
            ),
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

    fn lsp_params(params: impl Serialize, description: &'static str) -> anyhow::Result<Value> {
        serde_json::to_value(params).with_context(|| format!("Serializing {description} failed"))
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
        if notification.method() != SERVER_STATUS_METHOD {
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

enum ReadinessNotification {
    Ready,
    Failed(String),
    Ignore,
}
