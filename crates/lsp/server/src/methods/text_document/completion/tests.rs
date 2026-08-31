//! Completion behavior that depends on the LSP transport, ordered ingress, and engine scheduling.
//!
//! The scenarios come first. `CompletionLspFixture` below keeps their protocol setup and timing
//! control out of the test logic.

use std::{path::PathBuf, sync::Arc, time::Duration};

use futures::StreamExt as _;
use rg_lsp_proto::{
    CodeActionRequestContext, CompletionClientCapabilities, DocumentPositionSnapshot,
    DocumentRangeSnapshot, EditorDocumentSnapshot, EngineConfig, EngineResult, EngineService,
    EngineServiceClient, GlobalPositionSnapshot, QueryError, QueryScope, QueryValue, SaveProposal,
    SavedProjectChanges,
};
use rg_std::NormalizedPathBuf;
use tarpc::{
    client::Config as TarpcClientConfig,
    context,
    server::{BaseChannel, Channel as _},
};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader, DuplexStream},
    sync::{Notify, mpsc, oneshot},
    task::JoinHandle,
};
use tower_lsp_server::{
    LanguageServer, LspService, Server,
    jsonrpc::Result as LspResult,
    ls_types::{
        CompletionItem, CompletionItemKind, CompletionParams, CompletionResponse,
        CompletionTextEdit, DidChangeTextDocumentParams, DidOpenTextDocumentParams,
        DocumentHighlight, DocumentSymbol, Hover, InitializeParams, InitializeResult,
        InitializedParams, InlayHint, Location, Position, Range, TextEdit, Uri, WorkspaceEdit,
        WorkspaceSymbol,
    },
};

use super::completion;
use crate::{
    completion_scheduler::CompletionScheduler,
    engine_client::EngineClient,
    engine_registry::OpenDocumentRoute,
    ingress::{self, EditorIngress, EditorStateHandle, LifecycleEvent},
    inlay_refresher::InlayRefresher,
    methods::{CompletionMethodContext, DocumentMethodContext},
    recent_editor_saves::RecentEditorSaves,
    tests::synthetic_test_path,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(2);

#[tokio::test(flavor = "current_thread")]
async fn did_change_retries_completion_at_rebased_position() {
    let mut lsp = CompletionLspFixture::open("impl RwLo|").await;
    let request = lsp.request_completion().await;

    let mut first_attempt = lsp.expect_attempt("impl RwLo|").await;
    lsp.type_at_cursor("ck").await;
    first_attempt.expect_cancelled().await;

    let second_attempt = lsp.expect_attempt("impl RwLock|").await;
    second_attempt.complete();

    let response = lsp.expect_completion(request).await;
    let CompletionResponse::List(response) = response else {
        panic!("completion response should be a list");
    };
    assert!(!response.is_incomplete);
    let [item] = response.items.as_slice() else {
        panic!("completion response should contain one item");
    };
    assert_eq!(item.label, "RwLock");
    assert_eq!(item.kind, Some(CompletionItemKind::STRUCT));

    let Some(CompletionTextEdit::Edit(primary_edit)) = &item.text_edit else {
        panic!("completion item should replace the identifier at the moved cursor");
    };
    assert_eq!(
        primary_edit.range,
        Range::new(Position::new(0, 5), Position::new(0, 11))
    );
    let [additional_edit] = item
        .additional_text_edits
        .as_deref()
        .expect("completion item should add its import")
    else {
        panic!("completion item should contain one additional edit");
    };

    // Both edits must describe the final target document revision, not the shorter text from the
    // first attempt.
    // attempt. Applying them in source order produces the expected final document.
    let mut applied = "impl RwLock".to_string();
    applied.replace_range(5..11, &primary_edit.new_text);
    applied.insert_str(0, &additional_edit.new_text);
    assert_eq!(applied, "use std::sync::RwLock;\nimpl RwLock");

    lsp.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_completion_does_not_restart_after_did_change() {
    let mut lsp = CompletionLspFixture::open("impl RwLock|").await;
    let request = lsp.request_completion().await;
    let mut attempt = lsp.expect_attempt("impl RwLock|").await;

    lsp.cancel(request).await;
    lsp.expect_cancelled_response(request).await;
    attempt.expect_cancelled().await;

    lsp.type_at_cursor("X").await;
    lsp.expect_no_attempt().await;

    lsp.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn sibling_change_does_not_cancel_or_invalidate_completion() {
    let mut lsp = CompletionLspFixture::open("impl RwLock|").await;
    let sibling = lsp.open_sibling("pub struct Before;").await;
    let request = lsp.request_completion().await;
    let attempt = lsp.expect_attempt("impl RwLock|").await;

    lsp.change_sibling(&sibling, "pub struct After;").await;
    attempt.expect_running();
    attempt.complete();

    let response = lsp.expect_completion(request).await;
    let CompletionResponse::List(response) = response else {
        panic!("completion response should be a list");
    };
    assert_eq!(response.items[0].label, "RwLock");

    lsp.shutdown().await;
}

/// A real LSP transport around the completion handler and a controllable engine RPC.
///
/// Test scenarios use source text with `|` at the cursor. The fixture handles JSON-RPC framing,
/// document versions, timeouts, and server shutdown so each test can show only the order of editor
/// messages and semantic attempts that matters to it.
struct CompletionLspFixture {
    client_input: DuplexStream,
    client_output: BufReader<DuplexStream>,
    server: JoinHandle<()>,
    attempts: mpsc::UnboundedReceiver<ObservedCompletionAttempt>,
    opened: Arc<Notify>,
    changed: Arc<Notify>,
    workspace: PathBuf,
    uri: Uri,
    cursor: Position,
    version: i32,
    next_request_id: i64,
}

impl CompletionLspFixture {
    async fn open(marked_text: &str) -> Self {
        let (text, cursor) = Self::text_and_cursor(marked_text);
        let (engine_client, attempts) = GatedCompletionEngine::spawn();
        let editor = EditorStateHandle::default();
        let scheduler = CompletionScheduler::default();
        let opened = Arc::new(Notify::new());
        let changed = Arc::new(Notify::new());
        let (service, socket) = LspService::new({
            let engine_client = engine_client.clone();
            let opened = Arc::clone(&opened);
            let changed = Arc::clone(&changed);
            move |_| RawCompletionBackend {
                engine_client,
                opened,
                changed,
            }
        });
        let service = EditorIngress::new(
            service,
            editor,
            InlayRefresher::default(),
            RecentEditorSaves::default(),
            scheduler,
        );
        let (client_input, server_input) = tokio::io::duplex(64 * 1024);
        let (server_output, client_output) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(Server::new(server_input, server_output, socket).serve(service));
        let workspace = synthetic_test_path("workspace");
        let path = workspace.join("src/lib.rs");
        let workspace_uri = rg_lsp_proto::path_to_file_uri(&workspace)
            .expect("test workspace should convert to URI");
        let uri =
            rg_lsp_proto::path_to_file_uri(&path).expect("test document should convert to URI");
        let mut fixture = Self {
            client_input,
            client_output: BufReader::new(client_output),
            server,
            attempts,
            opened,
            changed,
            workspace,
            uri,
            cursor,
            version: 1,
            next_request_id: 2,
        };

        fixture
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "capabilities": {},
                    "rootUri": workspace_uri.as_str(),
                    "workspaceFolders": [{ "uri": workspace_uri.as_str(), "name": "workspace" }]
                }
            }))
            .await;
        let initialized = fixture.receive().await;
        assert_eq!(initialized["id"], 1);
        assert!(initialized.get("error").is_none());

        fixture
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "method": "initialized",
                "params": {}
            }))
            .await;
        fixture
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": {
                    "textDocument": {
                        "uri": fixture.uri.as_str(),
                        "languageId": "rust",
                        "version": fixture.version,
                        "text": text
                    }
                }
            }))
            .await;
        tokio::time::timeout(TEST_TIMEOUT, fixture.opened.notified())
            .await
            .expect("didOpen should publish the test route");

        fixture
    }

    async fn request_completion(&mut self) -> i64 {
        let id = self.next_id();
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": self.uri.as_str() },
                "position": self.cursor
            }
        }))
        .await;
        id
    }

    async fn open_sibling(&mut self, text: &str) -> Uri {
        let path = self.workspace.join("src/sibling.rs");
        let uri = rg_lsp_proto::path_to_file_uri(path).expect("sibling path should convert to URI");
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri.as_str(),
                    "languageId": "rust",
                    "version": 1,
                    "text": text
                }
            }
        }))
        .await;
        tokio::time::timeout(TEST_TIMEOUT, self.opened.notified())
            .await
            .expect("sibling didOpen should publish its route");
        uri
    }

    async fn change_sibling(&mut self, uri: &Uri, text: &str) {
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri.as_str(), "version": 2 },
                "contentChanges": [{ "text": text }]
            }
        }))
        .await;
        tokio::time::timeout(TEST_TIMEOUT, self.changed.notified())
            .await
            .expect("sibling didChange should reach its handler");
    }

    async fn type_at_cursor(&mut self, text: &str) {
        self.version += 1;
        let position = self.cursor;
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": self.uri.as_str(), "version": self.version },
                "contentChanges": [{
                    "range": { "start": position, "end": position },
                    "text": text
                }]
            }
        }))
        .await;
        tokio::time::timeout(TEST_TIMEOUT, self.changed.notified())
            .await
            .expect("didChange should reach its handler");
        Self::advance_position(&mut self.cursor, text);
    }

    async fn expect_attempt(&mut self, marked_text: &str) -> PendingCompletionAttempt {
        let (expected_text, expected_position) = Self::text_and_cursor(marked_text);
        let observed = tokio::time::timeout(TEST_TIMEOUT, self.attempts.recv())
            .await
            .expect("semantic completion attempt should start")
            .expect("test engine should report its completion attempt");
        assert_eq!(observed.input.position(), expected_position);
        assert_eq!(observed.input.document().text(), expected_text);
        PendingCompletionAttempt {
            release: observed.release,
        }
    }

    async fn expect_completion(&mut self, id: i64) -> CompletionResponse {
        let response = self.receive().await;
        assert_eq!(response["id"], id);
        assert!(response.get("error").is_none());
        serde_json::from_value(response["result"].clone())
            .expect("LSP response should contain a completion value")
    }

    async fn cancel(&mut self, id: i64) {
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "$/cancelRequest",
            "params": { "id": id }
        }))
        .await;
    }

    async fn expect_cancelled_response(&mut self, id: i64) {
        let response = self.receive().await;
        assert_eq!(response["id"], id);
        assert!(
            response.get("error").is_some(),
            "cancelled request must not publish a completion value"
        );
    }

    async fn expect_no_attempt(&mut self) {
        tokio::task::yield_now().await;
        match self.attempts.try_recv() {
            Err(mpsc::error::TryRecvError::Empty) => {}
            Err(mpsc::error::TryRecvError::Disconnected) => {
                panic!("test engine attempt channel closed unexpectedly")
            }
            Ok(_) => panic!("editor change restarted a cancelled completion request"),
        }
    }

    async fn shutdown(mut self) {
        let id = self.next_id();
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "shutdown",
            "params": null
        }))
        .await;
        let response = self.receive().await;
        assert_eq!(response["id"], id);
        assert!(response.get("error").is_none());
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "exit"
        }))
        .await;
        drop(self.client_input);

        tokio::time::timeout(TEST_TIMEOUT, self.server)
            .await
            .expect("raw LSP server should stop after exit")
            .expect("raw LSP server task should not panic");
    }

    async fn send(&mut self, value: serde_json::Value) {
        let body = serde_json::to_vec(&value).expect("LSP test message should serialize");
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.client_input
            .write_all(header.as_bytes())
            .await
            .expect("LSP test header should write");
        self.client_input
            .write_all(&body)
            .await
            .expect("LSP test body should write");
        self.client_input
            .flush()
            .await
            .expect("LSP test message should flush");
    }

    async fn receive(&mut self) -> serde_json::Value {
        let mut content_length = None;
        loop {
            let mut line = String::new();
            tokio::time::timeout(TEST_TIMEOUT, self.client_output.read_line(&mut line))
                .await
                .expect("LSP response should arrive")
                .expect("LSP test header should read");
            assert!(
                !line.is_empty(),
                "LSP output ended before a complete header"
            );
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                content_length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .expect("content length should be numeric"),
                );
            }
        }

        let mut body = vec![0; content_length.expect("LSP output should include content length")];
        tokio::time::timeout(TEST_TIMEOUT, self.client_output.read_exact(&mut body))
            .await
            .expect("complete LSP response body should arrive")
            .expect("LSP test body should read");
        serde_json::from_slice(&body).expect("LSP output should contain JSON")
    }

    fn next_id(&mut self) -> i64 {
        let id = self.next_request_id;
        self.next_request_id += 1;
        id
    }

    fn text_and_cursor(marked_text: &str) -> (String, Position) {
        let marker = marked_text
            .find('|')
            .expect("completion fixture text should contain a cursor marker");
        assert!(
            !marked_text[marker + 1..].contains('|'),
            "completion fixture text should contain exactly one cursor marker"
        );
        let before = &marked_text[..marker];
        let line = u32::try_from(before.matches('\n').count())
            .expect("fixture line number should fit into u32");
        let character = u32::try_from(
            before
                .rsplit_once('\n')
                .map_or(before, |(_, line)| line)
                .encode_utf16()
                .count(),
        )
        .expect("fixture character offset should fit into u32");
        let mut text = marked_text.to_string();
        text.remove(marker);
        (text, Position::new(line, character))
    }

    fn advance_position(position: &mut Position, text: &str) {
        for character in text.chars() {
            if character == '\n' {
                position.line += 1;
                position.character = 0;
            } else {
                position.character += u32::try_from(character.len_utf16())
                    .expect("one UTF-16 character width should fit into u32");
            }
        }
    }
}

struct PendingCompletionAttempt {
    release: oneshot::Sender<()>,
}

impl PendingCompletionAttempt {
    fn complete(self) {
        self.release
            .send(())
            .expect("completion attempt should still be running");
    }

    async fn expect_cancelled(&mut self) {
        tokio::time::timeout(TEST_TIMEOUT, self.release.closed())
            .await
            .expect("completion attempt should be cancelled");
    }

    fn expect_running(&self) {
        assert!(
            !self.release.is_closed(),
            "sibling editor state must not cancel target-only completion"
        );
    }
}

#[derive(Clone)]
struct RawCompletionBackend {
    engine_client: EngineClient,
    opened: Arc<Notify>,
    changed: Arc<Notify>,
}

impl LanguageServer for RawCompletionBackend {
    async fn initialize(&self, _: InitializeParams) -> LspResult<InitializeResult> {
        Ok(crate::methods::initialize())
    }

    async fn initialized(&self, _: InitializedParams) {}

    async fn shutdown(&self) -> LspResult<()> {
        Ok(())
    }

    async fn did_open(&self, _: DidOpenTextDocumentParams) {
        let Some(LifecycleEvent::Open { document, route }) = ingress::lifecycle_event().await
        else {
            panic!("raw didOpen should carry its ingress lifecycle event");
        };
        route.publish(Ok(Some(OpenDocumentRoute::new(
            self.engine_client.clone(),
            NormalizedPathBuf::from_absolute(document.path())
                .expect("opened test document path should normalize"),
        ))));
        self.opened.notify_one();
    }

    async fn did_change(&self, _: DidChangeTextDocumentParams) {
        self.changed.notify_one();
    }

    async fn completion(&self, params: CompletionParams) -> LspResult<Option<CompletionResponse>> {
        let captured = ingress::document_request()
            .expect("raw completion should run inside ordered ingress")
            .map_err(|unavailable| crate::methods::temporarily_unavailable(unavailable.reason()))?;
        let request = ingress::completion_request().ok_or_else(|| {
            crate::methods::internal_error(anyhow::anyhow!(
                "raw completion has no logical request token"
            ))
        })?;
        let document = DocumentMethodContext::new(self.engine_client.clone(), captured);
        let context = CompletionMethodContext::new(
            document,
            request,
            CompletionClientCapabilities::default(),
        );
        completion(context, params).await
    }
}

#[derive(Clone)]
struct GatedCompletionEngine {
    attempts: mpsc::UnboundedSender<ObservedCompletionAttempt>,
}

impl GatedCompletionEngine {
    fn spawn() -> (
        EngineClient,
        mpsc::UnboundedReceiver<ObservedCompletionAttempt>,
    ) {
        let (attempts, attempt_rx) = mpsc::unbounded_channel();
        let engine = Self { attempts };
        let (client_transport, server_transport) = tarpc::transport::channel::unbounded();
        let server = BaseChannel::with_defaults(server_transport);
        tokio::spawn(
            server
                .execute(engine.serve())
                .for_each(|response| async move {
                    tokio::spawn(response);
                }),
        );
        let client =
            EngineServiceClient::new(TarpcClientConfig::default(), client_transport).spawn();
        (EngineClient::new(client), attempt_rx)
    }
}

struct ObservedCompletionAttempt {
    input: DocumentPositionSnapshot,
    release: oneshot::Sender<()>,
}

impl EngineService for GatedCompletionEngine {
    async fn completion(
        self,
        _: context::Context,
        input: DocumentPositionSnapshot,
        _: CompletionClientCapabilities,
    ) -> Result<QueryValue<Vec<CompletionItem>>, QueryError> {
        let (release, released) = oneshot::channel();
        self.attempts
            .send(ObservedCompletionAttempt {
                input: input.clone(),
                release,
            })
            .expect("test should observe every completion RPC");
        let _ = released.await;

        let scope = QueryScope::TargetDocument(input.document().target().clone());
        Ok(QueryValue::new(
            vec![CompletionItem {
                label: "RwLock".to_string(),
                kind: Some(CompletionItemKind::STRUCT),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                    Range::new(Position::new(0, 5), input.position()),
                    "RwLock".to_string(),
                ))),
                additional_text_edits: Some(vec![TextEdit::new(
                    Range::new(Position::new(0, 0), Position::new(0, 0)),
                    "use std::sync::RwLock;\n".to_string(),
                )]),
                ..CompletionItem::default()
            }],
            scope,
        ))
    }

    // This deliberately narrow test engine implements the remaining RPC surface only so the
    // generated tarpc client is real. Calling any method other than completion is a test bug.
    async fn initialize(
        self,
        _: context::Context,
        _: PathBuf,
        _: EngineConfig,
    ) -> EngineResult<()> {
        panic!("test engine only supports completion")
    }

    async fn initialized(self, _: context::Context) -> EngineResult<()> {
        panic!("test engine only supports completion")
    }

    async fn set_deferred_indexing_priority(
        self,
        _: context::Context,
        _: PathBuf,
        _: bool,
    ) -> EngineResult<()> {
        // Raw didOpen/didClose in these tests may carry the ordinary best-effort scheduler hint.
        Ok(())
    }

    async fn did_save(self, _: context::Context, _: SaveProposal) -> EngineResult<u64> {
        panic!("test engine only supports completion")
    }

    async fn external_project_changes(
        self,
        _: context::Context,
        _: SavedProjectChanges,
    ) -> EngineResult<()> {
        panic!("test engine only supports completion")
    }

    async fn goto_definition(
        self,
        _: context::Context,
        _: GlobalPositionSnapshot,
    ) -> Result<QueryValue<Vec<Location>>, QueryError> {
        panic!("test engine only supports completion")
    }

    async fn goto_type_definition(
        self,
        _: context::Context,
        _: GlobalPositionSnapshot,
    ) -> Result<QueryValue<Vec<Location>>, QueryError> {
        panic!("test engine only supports completion")
    }

    async fn goto_implementation(
        self,
        _: context::Context,
        _: GlobalPositionSnapshot,
    ) -> Result<QueryValue<Vec<Location>>, QueryError> {
        panic!("test engine only supports completion")
    }

    async fn references(
        self,
        _: context::Context,
        _: GlobalPositionSnapshot,
        _: bool,
    ) -> Result<QueryValue<Vec<Location>>, QueryError> {
        panic!("test engine only supports completion")
    }

    async fn prepare_rename(
        self,
        _: context::Context,
        _: GlobalPositionSnapshot,
    ) -> Result<QueryValue<Option<tower_lsp_server::ls_types::PrepareRenameResponse>>, QueryError>
    {
        panic!("test engine only supports completion")
    }

    async fn rename(
        self,
        _: context::Context,
        _: GlobalPositionSnapshot,
        _: String,
    ) -> Result<QueryValue<Option<WorkspaceEdit>>, QueryError> {
        panic!("test engine only supports completion")
    }

    async fn document_highlight(
        self,
        _: context::Context,
        _: DocumentPositionSnapshot,
    ) -> Result<QueryValue<Vec<DocumentHighlight>>, QueryError> {
        panic!("test engine only supports completion")
    }

    async fn hover(
        self,
        _: context::Context,
        _: DocumentPositionSnapshot,
    ) -> Result<QueryValue<Option<Hover>>, QueryError> {
        panic!("test engine only supports completion")
    }

    async fn code_action(
        self,
        _: context::Context,
        _: DocumentRangeSnapshot,
        _: CodeActionRequestContext,
    ) -> Result<QueryValue<Vec<tower_lsp_server::ls_types::CodeAction>>, QueryError> {
        panic!("test engine only supports completion")
    }

    async fn formatting(
        self,
        _: context::Context,
        _: EditorDocumentSnapshot,
    ) -> Result<QueryValue<Option<Vec<TextEdit>>>, QueryError> {
        panic!("test engine only supports completion")
    }

    async fn document_symbol(
        self,
        _: context::Context,
        _: EditorDocumentSnapshot,
    ) -> Result<QueryValue<Vec<DocumentSymbol>>, QueryError> {
        panic!("test engine only supports completion")
    }

    async fn inlay_hint(
        self,
        _: context::Context,
        _: DocumentRangeSnapshot,
    ) -> Result<QueryValue<Vec<InlayHint>>, QueryError> {
        panic!("test engine only supports completion")
    }

    async fn workspace_symbol(
        self,
        _: context::Context,
        _: String,
    ) -> Result<QueryValue<Vec<WorkspaceSymbol>>, QueryError> {
        panic!("test engine only supports completion")
    }

    async fn reindex_workspace(self, _: context::Context) -> EngineResult<()> {
        panic!("test engine only supports completion")
    }

    async fn shutdown(self, _: context::Context) -> EngineResult<()> {
        panic!("test engine only supports completion")
    }
}
