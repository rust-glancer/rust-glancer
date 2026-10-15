use std::{fmt::Write as _, time::Duration};

use expect_test::Expect;
use futures::{SinkExt as _, StreamExt as _};
use serde_json::Value;
use tower::{Service as _, ServiceExt as _};
use tower_lsp_server::{
    ClientSocket, LanguageServer, LspService,
    gen_lsp_types::{InitializeParams, InitializeResult},
    jsonrpc::{Request, Response, Result},
};

use crate::client_status::{ClientStatusCapabilities, ClientStatusPublisher};

/// Record complete server payloads and explicit client replies. Tests name generated progress
/// tokens, while engine-supplied tokens stay literal. Request IDs are numbered by first appearance.
pub(super) struct ProtocolClient {
    socket: ClientSocket,
    events: Vec<Value>,
    tokens: Vec<(Value, String)>,
}

impl ProtocolClient {
    pub(super) async fn initialized(
        capabilities: ClientStatusCapabilities,
    ) -> (ClientStatusPublisher, Self) {
        let (mut service, socket) = LspService::new(move |client| TestBackend {
            client_status: ClientStatusPublisher::new(client, capabilities),
        });
        service
            .ready()
            .await
            .expect("test LSP service should become ready")
            .call(
                Request::build("initialize")
                    .params(serde_json::json!({"capabilities": {}}))
                    .id(1)
                    .finish(),
            )
            .await
            .expect("initialize should reach the service")
            .expect("initialize should return a response");

        // The publisher is the only remaining sender once this service is dropped. Moving it
        // into the publishing future lets the client drain until EOF after the last event.
        let publisher = service.inner().client_status.clone();
        (
            publisher,
            Self {
                socket,
                events: Vec::new(),
                tokens: Vec::new(),
            },
        )
    }

    /// Record preceding notifications too, so a request cannot hide changes to their order.
    pub(super) async fn next_request(&mut self) -> Request {
        loop {
            let event = self.receive().await.expect("server should send a request");
            if event.id().is_some() {
                return event;
            }
        }
    }

    pub(super) fn name_progress_token(&mut self, request: &Request, name: &str) {
        assert_eq!(request.method(), "window/workDoneProgress/create");
        let token = request
            .params()
            .and_then(|params| params.get("token"))
            .expect("progress creation should name its token");
        assert!(
            token.is_string() || token.is_i64(),
            "progress token should be a string or integer"
        );
        assert!(
            self.tokens
                .iter()
                .all(|(value, alias)| value != token && alias != name),
            "each progress operation needs a distinct token and transcript name"
        );
        self.tokens.push((token.clone(), name.to_string()));
    }

    pub(super) async fn acknowledge(&mut self, request: Request) {
        let id = request
            .id()
            .cloned()
            .expect("acknowledged event should be a request");
        let response = Response::from_ok(id, Value::Null);
        self.socket
            .send(response.clone())
            .await
            .expect("client should acknowledge the request");
        self.events
            .push(serde_json::to_value(response).expect("response should serialize"));
    }

    /// The publisher closes the channel when its future finishes. Drain every remaining event
    /// rather than choosing an expected count or assuming an idle interval means completion.
    pub(super) async fn drain(&mut self) {
        while let Some(event) = self.receive().await {
            assert!(
                event.id().is_none(),
                "acknowledge server requests explicitly before draining"
            );
        }
    }

    async fn receive(&mut self) -> Option<Request> {
        let event = tokio::time::timeout(Duration::from_secs(5), self.socket.next())
            .await
            .expect("protocol exchange should make progress");
        if let Some(event) = &event {
            self.events
                .push(serde_json::to_value(event).expect("server event should serialize"));
        }
        event
    }

    pub(super) fn check(&self, expected: Expect) {
        let mut output = String::new();
        let mut ids = Vec::new();
        for event in &self.events {
            let mut event = event.clone();
            let object = event
                .as_object_mut()
                .expect("JSON-RPC message is an object");
            // JSON-RPC's constant version and generated ID spelling add no scenario information.
            // Keep the request/reply identity and every application payload field.
            object.remove("jsonrpc");
            let id = object.remove("id").map(|id| {
                if let Some(index) = ids.iter().position(|known| *known == id) {
                    index
                } else {
                    ids.push(id);
                    ids.len() - 1
                }
            });
            if let Some(method) = object.remove("method") {
                let method = method.as_str().expect("method is a string");
                let kind = id.map_or_else(|| "notify".to_string(), |id| format!("request #{id}"));
                if let Some(params) = object.get_mut("params") {
                    if let Some(token) = params.get_mut("token")
                        && let Some((_, name)) =
                            self.tokens.iter().find(|(value, _)| value == token)
                    {
                        *token = Value::String(name.clone());
                    }
                    params.sort_all_objects();
                    writeln!(output, "{kind} {method} {params}").expect("string write");
                } else {
                    writeln!(output, "{kind} {method} <no params>").expect("string write");
                }
            } else {
                let id = id.expect("reply should have a request ID");
                event.sort_all_objects();
                writeln!(output, "reply #{id} {event}").expect("string write");
            }
        }
        expected.assert_eq(&output);
    }
}

#[derive(Debug)]
pub(super) struct TestBackend {
    pub(super) client_status: ClientStatusPublisher,
}

impl LanguageServer for TestBackend {
    async fn initialize(&self, _params: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult::default())
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}
