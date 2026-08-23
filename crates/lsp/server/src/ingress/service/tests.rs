use std::{
    convert::Infallible,
    io::Cursor,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};

use futures::{future::BoxFuture, sink, stream};
use serde_json::json;
use test_fixture::fixture_crate;
use tokio::sync::Notify;
use tower::Service;
use tower_lsp_server::{
    Loopback, Server,
    jsonrpc::{Request, Response},
    ls_types::Uri,
};

use crate::{
    completion_scheduler::CompletionScheduler, ingress::EditorStateHandle,
    inlay_refresher::InlayRefresher, recent_editor_saves::RecentEditorSaves,
};

use super::{EditorIngress, completion_request, document_request};

#[tokio::test(flavor = "current_thread")]
async fn transport_calls_service_in_wire_order_while_futures_finish_in_reverse() {
    let state = Arc::new(OrderingState {
        calls: Mutex::new(Vec::new()),
        completions: Mutex::new(Vec::new()),
        next_completion: AtomicUsize::new(3),
        completion_changed: Notify::new(),
    });
    let service = ReverseCompletionService {
        state: Arc::clone(&state),
    };
    let input = framed_notifications(&["test/first", "test/second", "test/third"]);

    Server::new(Cursor::new(input), Vec::new(), EmptyLoopback)
        .serve(service)
        .await;

    assert_eq!(
        *state.calls.lock().expect("call log mutex should be usable"),
        ["test/first", "test/second", "test/third"]
    );
    assert_eq!(
        *state
            .completions
            .lock()
            .expect("completion log mutex should be usable"),
        ["test/third", "test/second", "test/first"]
    );
}

#[tokio::test]
async fn later_request_keeps_incrementally_changed_text_when_futures_finish_in_reverse() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let inner = CapturingService {
        captured: Arc::clone(&captured),
    };
    let mut service = EditorIngress::new(
        inner,
        EditorStateHandle::default(),
        InlayRefresher::default(),
        RecentEditorSaves::default(),
        CompletionScheduler::default(),
    );
    let uri = document_uri();

    let open = service.call(
        Request::build("textDocument/didOpen")
            .params(json!({
                "textDocument": {
                    "uri": uri.as_str(),
                    "languageId": "rust",
                    "version": 1,
                    "text": "fn opened() {}"
                }
            }))
            .finish(),
    );
    let change = service.call(
        Request::build("textDocument/didChange")
            .params(json!({
                "textDocument": { "uri": uri.as_str(), "version": 2 },
                "contentChanges": [{
                    "range": {
                        "start": { "line": 0, "character": 3 },
                        "end": { "line": 0, "character": 9 }
                    },
                    "text": "changed"
                }]
            }))
            .finish(),
    );
    let query = service.call(
        Request::build("textDocument/hover")
            .id(1_i64)
            .params(json!({
                "textDocument": { "uri": uri.as_str() },
                "position": { "line": 0, "character": 3 }
            }))
            .finish(),
    );

    // The later query completes first. Each query must still read the document snapshot chosen
    // for it in `Service::call`, including the text from changes that arrived before it.
    query.await.expect("query future should complete");
    change.await.expect("change future should complete");
    open.await.expect("open future should complete");

    assert_eq!(
        *captured
            .lock()
            .expect("captured text mutex should be usable"),
        [("fn changed() {}".to_string(), (0, 3))]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn completion_ownership_is_decided_in_wire_order_before_handlers_are_polled() {
    let observations = Arc::new(Mutex::new(Vec::new()));
    let inner = CompletionOwnershipService {
        observations: Arc::clone(&observations),
    };
    let mut service = EditorIngress::new(
        inner,
        EditorStateHandle::default(),
        InlayRefresher::default(),
        RecentEditorSaves::default(),
        CompletionScheduler::default(),
    );
    let uri = document_uri();

    service
        .call(
            Request::build("textDocument/didOpen")
                .params(json!({
                    "textDocument": {
                        "uri": uri.as_str(),
                        "languageId": "rust",
                        "version": 1,
                        "text": "impl Source"
                    }
                }))
                .finish(),
        )
        .await
        .expect("open handler should finish");
    let older = service.call(completion_request_message(&uri, 1, 9));
    let newer = service.call(completion_request_message(&uri, 2, 11));

    // Poll in reverse to prove each snapshot was chosen in `Service::call`, not according to
    // the order in which handler futures happened to run.
    newer.await.expect("newer completion should finish");
    older.await.expect("older completion should finish");

    let mut observations = observations
        .lock()
        .expect("completion observation mutex should be usable")
        .clone();
    observations.sort_unstable();
    assert_eq!(observations, [(9, true), (11, false)]);
}

#[test]
fn save_echo_is_recorded_before_any_handler_future_is_polled() {
    let fixture = fixture_crate(
        r#"
        //- /src/lib.rs
        pub fn saved() {}
        "#,
    );
    let path = fixture.path("src/lib.rs");
    let uri = Uri::from_file_path(&path).expect("fixture path should convert to URI");
    let recent_editor_saves = RecentEditorSaves::default();
    let inner = CapturingService {
        captured: Arc::new(Mutex::new(Vec::new())),
    };
    let mut service = EditorIngress::new(
        inner,
        EditorStateHandle::default(),
        InlayRefresher::default(),
        recent_editor_saves.clone(),
        CompletionScheduler::default(),
    );

    let _open = service.call(
        Request::build("textDocument/didOpen")
            .params(json!({
                "textDocument": {
                    "uri": uri.as_str(),
                    "languageId": "rust",
                    "version": 1,
                    "text": "pub fn saved() {}\n"
                }
            }))
            .finish(),
    );
    let _save = service.call(
        Request::build("textDocument/didSave")
            .params(json!({
                "textDocument": { "uri": uri.as_str() },
                "text": "pub fn saved() {}\n"
            }))
            .finish(),
    );

    assert!(
        recent_editor_saves.saves_to_process(vec![path]).is_empty(),
        "watcher filtering must observe the save before async lifecycle routing starts",
    );
}

#[derive(Debug)]
struct OrderingState {
    calls: Mutex<Vec<&'static str>>,
    completions: Mutex<Vec<&'static str>>,
    next_completion: AtomicUsize,
    completion_changed: Notify,
}

#[derive(Debug)]
struct ReverseCompletionService {
    state: Arc<OrderingState>,
}

impl Service<Request> for ReverseCompletionService {
    type Response = Option<Response>;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let (method, rank) = match request.method() {
            "test/first" => ("test/first", 1),
            "test/second" => ("test/second", 2),
            "test/third" => ("test/third", 3),
            method => panic!("unexpected test method {method}"),
        };
        self.state
            .calls
            .lock()
            .expect("call log mutex should be usable")
            .push(method);
        let state = Arc::clone(&self.state);

        Box::pin(async move {
            loop {
                let notified = state.completion_changed.notified();
                if state.next_completion.load(Ordering::Relaxed) == rank {
                    state
                        .completions
                        .lock()
                        .expect("completion log mutex should be usable")
                        .push(method);
                    state.next_completion.fetch_sub(1, Ordering::Relaxed);
                    state.completion_changed.notify_waiters();
                    return Ok(None);
                }
                notified.await;
            }
        })
    }
}

#[derive(Debug)]
struct CapturingService {
    captured: Arc<Mutex<Vec<CapturedQuery>>>,
}

#[derive(Debug)]
struct CompletionOwnershipService {
    observations: Arc<Mutex<Vec<(u64, bool)>>>,
}

impl Service<Request> for CompletionOwnershipService {
    type Response = Option<Response>;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let character = request
            .params()
            .and_then(|params| params.get("position")?.get("character")?.as_u64());
        let observations = Arc::clone(&self.observations);
        Box::pin(async move {
            if let Some(character) = character {
                let request = completion_request()
                    .expect("completion should carry an ingress ownership token");
                observations
                    .lock()
                    .expect("completion observation mutex should be usable")
                    .push((character, request.is_replaced()));
            }
            Ok(None)
        })
    }
}

type CapturedQuery = (String, (u64, u64));

impl Service<Request> for CapturingService {
    type Response = Option<Response>;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let is_query = request.method() == "textDocument/hover";
        let position = request.params().and_then(|params| {
            Some((
                params.get("position")?.get("line")?.as_u64()?,
                params.get("position")?.get("character")?.as_u64()?,
            ))
        });
        let captured = Arc::clone(&self.captured);
        Box::pin(async move {
            if is_query {
                let document = document_request()
                    .expect("query should run inside an ingress envelope")
                    .expect("changed document should remain available");
                captured
                    .lock()
                    .expect("captured text mutex should be usable")
                    .push((
                        document.document().text().to_string(),
                        position.expect("query should capture its position before polling"),
                    ));
            }
            Ok(None)
        })
    }
}

#[derive(Debug)]
struct EmptyLoopback;

impl Loopback for EmptyLoopback {
    type RequestStream = stream::Empty<Request>;
    type ResponseSink = sink::Drain<Response>;

    fn split(self) -> (Self::RequestStream, Self::ResponseSink) {
        (stream::empty(), sink::drain())
    }
}

fn framed_notifications(methods: &[&str]) -> Vec<u8> {
    methods
        .iter()
        .flat_map(|method| {
            let body = format!(r#"{{"jsonrpc":"2.0","method":"{method}"}}"#);
            format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
        })
        .collect()
}

/// `Uri::from_file_path` requires an absolute host path, which on Windows means
/// a drive letter, so the synthetic document path is anchored at the temp dir.
fn document_uri() -> Uri {
    let path = std::env::temp_dir()
        .join("workspace")
        .join("src")
        .join("lib.rs");
    Uri::from_file_path(path).expect("test path should convert to URI")
}

fn completion_request_message(uri: &Uri, id: i64, character: u32) -> Request {
    Request::build("textDocument/completion")
        .id(id)
        .params(json!({
            "textDocument": { "uri": uri.as_str() },
            "position": { "line": 0, "character": character }
        }))
        .finish()
}
