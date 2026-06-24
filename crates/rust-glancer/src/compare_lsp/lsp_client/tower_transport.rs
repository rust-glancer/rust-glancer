//! `tower-lsp-server` adapter for the client side of the comparison harness.
//!
//! `tower-lsp-server` owns the stdio framing and JSON-RPC message loop. This file adds the small
//! client-shaped layer the harness needs: enqueue outbound requests, correlate responses by ID,
//! and answer the server-to-client requests that language servers commonly send during startup.

use std::{
    collections::HashMap,
    fmt,
    future::{Ready, ready},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use futures::{
    Sink,
    channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded},
};
use serde_json::Value;
use tokio::{
    process,
    sync::{oneshot, watch},
};
use tower::Service;
use tower_lsp_server::{
    Loopback, Server,
    jsonrpc::{Id as JsonRpcId, Request as JsonRpcRequest, Response as JsonRpcResponse},
};

use super::{RequestOutcome, RpcError};

/// Request IDs that are waiting for a server response.
type PendingResponses = Arc<Mutex<HashMap<JsonRpcId, oneshot::Sender<JsonRpcResponse>>>>;

/// Async request/notification handle for one spawned LSP process.
///
/// The handle is intentionally small: server lifecycle code can send ordinary requests without
/// knowing whether the bytes on stdio are headers, JSON-RPC messages, or tower service calls.
pub(crate) struct TowerLspTransport {
    outbound: UnboundedSender<JsonRpcRequest>,
    pending: PendingResponses,
    transport_finished: watch::Receiver<bool>,
    serve_task: tokio::task::JoinHandle<()>,
    next_request_id: AtomicI64,
}

impl TowerLspTransport {
    /// Start the tower message loop on the child process pipes.
    pub(crate) fn spawn(reader: process::ChildStdout, writer: process::ChildStdin) -> Self {
        let (outbound, outbound_rx) = unbounded();
        let (finished_sender, transport_finished) = watch::channel(false);
        let pending = PendingResponses::default();
        let loopback = TowerLoopback {
            outbound: outbound_rx,
            pending: Arc::clone(&pending),
        };

        let serve_task = tokio::spawn(async move {
            Server::new(reader, writer, loopback)
                .serve(ClientRequestService)
                .await;
            let _ = finished_sender.send(true);
        });

        Self {
            outbound,
            pending,
            transport_finished,
            serve_task,
            next_request_id: AtomicI64::new(1),
        }
    }

    fn next_request_id(&self) -> JsonRpcId {
        JsonRpcId::Number(self.next_request_id.fetch_add(1, Ordering::Relaxed))
    }

    fn remove_pending(&self, id: &JsonRpcId) {
        self.pending
            .lock()
            .expect("pending response mutex should not be poisoned")
            .remove(id);
    }

    /// Wait for either the matching response, transport shutdown, or the request timeout.
    async fn wait_for_response(
        &self,
        id: &JsonRpcId,
        receiver: oneshot::Receiver<JsonRpcResponse>,
        timeout: Duration,
    ) -> RequestOutcome {
        let mut transport_finished = self.transport_finished.clone();
        if *transport_finished.borrow() {
            self.remove_pending(id);
            return RequestOutcome::TransportFailure {
                message: "tower LSP transport task stopped before response".to_string(),
            };
        }

        tokio::select! {
            response = receiver => {
                match response {
                    Ok(response) => Self::response_outcome(response),
                    Err(_error) => {
                        self.remove_pending(id);
                        RequestOutcome::TransportFailure {
                            message: "tower LSP response waiter disconnected".to_string(),
                        }
                    }
                }
            }
            changed = transport_finished.changed() => {
                self.remove_pending(id);
                let message = if changed.is_ok() {
                    "tower LSP transport task stopped before response"
                } else {
                    "tower LSP transport finished signal closed before response"
                };
                RequestOutcome::TransportFailure {
                    message: message.to_string(),
                }
            }
            () = tokio::time::sleep(timeout) => {
                self.remove_pending(id);
                RequestOutcome::Timeout
            }
        }
    }

    fn response_outcome(response: JsonRpcResponse) -> RequestOutcome {
        let (_id, body) = response.into_parts();
        match body {
            Ok(value) => RequestOutcome::Success(value),
            Err(error) => RequestOutcome::Error(RpcError {
                code: error.code.code(),
                message: error.message.into_owned(),
                data: error.data,
            }),
        }
    }
}

impl fmt::Debug for TowerLspTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TowerLspTransport")
            .field("serve_task_finished", &self.serve_task.is_finished())
            .finish_non_exhaustive()
    }
}

impl TowerLspTransport {
    pub(crate) async fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> RequestOutcome {
        if self.serve_task.is_finished() {
            return RequestOutcome::TransportFailure {
                message: "tower LSP transport task is not running".to_string(),
            };
        }

        let id = self.next_request_id();
        let request = JsonRpcRequest::build(method.to_string())
            .id(id.clone())
            .params(params)
            .finish();
        let (response_sender, response_receiver) = oneshot::channel();
        self.pending
            .lock()
            .expect("pending response mutex should not be poisoned")
            .insert(id.clone(), response_sender);

        if let Err(error) = self.outbound.unbounded_send(request) {
            self.remove_pending(&id);
            return RequestOutcome::TransportFailure {
                message: format!("failed to enqueue LSP request `{method}`: {error}"),
            };
        }

        self.wait_for_response(&id, response_receiver, timeout)
            .await
    }

    pub(crate) async fn notify(&mut self, method: &str, params: Value) -> anyhow::Result<()> {
        let notification = JsonRpcRequest::build(method.to_string())
            .params(params)
            .finish();
        self.outbound.unbounded_send(notification).map_err(|error| {
            anyhow::anyhow!("failed to enqueue LSP notification `{method}`: {error}")
        })
    }
}

impl Drop for TowerLspTransport {
    fn drop(&mut self) {
        self.serve_task.abort();
    }
}

struct TowerLoopback {
    outbound: UnboundedReceiver<JsonRpcRequest>,
    pending: PendingResponses,
}

/// Connects harness-issued requests to the tower server loop.
///
/// The library calls this a loopback because the server loop normally owns inbound client
/// requests. Here the harness is the client, so the "inbound" stream is the queue of requests we
/// want written to the child process.
impl Loopback for TowerLoopback {
    type RequestStream = UnboundedReceiver<JsonRpcRequest>;
    type ResponseSink = TowerResponseSink;

    fn split(self) -> (Self::RequestStream, Self::ResponseSink) {
        (
            self.outbound,
            TowerResponseSink {
                pending: self.pending,
            },
        )
    }
}

struct TowerResponseSink {
    pending: PendingResponses,
}

/// Routes server responses back to the request waiter that owns the matching JSON-RPC ID.
impl Sink<JsonRpcResponse> for TowerResponseSink {
    type Error = TowerTransportError;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, response: JsonRpcResponse) -> Result<(), Self::Error> {
        let id = response.id().clone();
        if let Some(sender) = self
            .pending
            .lock()
            .expect("pending response mutex should not be poisoned")
            .remove(&id)
        {
            let _ = sender.send(response);
        }
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

#[derive(Debug)]
struct ClientRequestService;

/// Minimal service for requests that the language server sends back to this harness.
///
/// The comparison command is not an editor, but servers may still ask the client for configuration
/// during initialization. Returning neutral values keeps both sides alive without teaching the
/// harness editor-specific behavior.
impl Service<JsonRpcRequest> for ClientRequestService {
    type Response = Option<JsonRpcResponse>;
    type Error = TowerTransportError;
    type Future = Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: JsonRpcRequest) -> Self::Future {
        let method = request.method().to_string();
        let id = request.id().cloned();
        let params = request.params().cloned();
        let Some(id) = id else {
            return ready(Ok(None));
        };

        // LSP expects one configuration result per requested item. `null` means "no setting
        // supplied", which is the least opinionated answer for this comparison client.
        let result = if method == "workspace/configuration" {
            let item_count = params
                .as_ref()
                .and_then(|params| params.get("items"))
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            Value::Array(vec![Value::Null; item_count])
        } else {
            Value::Null
        };

        ready(Ok(Some(JsonRpcResponse::from_ok(id, result))))
    }
}

#[derive(Debug)]
struct TowerTransportError;

impl fmt::Display for TowerTransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("tower LSP transport error")
    }
}

impl std::error::Error for TowerTransportError {}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use serde_json::json;

    use super::*;

    #[test]
    fn workspace_configuration_request_returns_one_null_per_item() {
        let mut service = ClientRequestService;
        let request = JsonRpcRequest::build("workspace/configuration")
            .id(1)
            .params(json!({
                "items": [
                    {"section": "rust-analyzer"},
                    {"section": "rust-glancer"}
                ],
            }))
            .finish();

        let response = block_on(service.call(request))
            .expect("client request should be handled")
            .expect("server-to-client request should receive a response");
        let (_id, result) = response.into_parts();

        assert_eq!(
            result.expect("workspace/configuration should succeed"),
            json!([null, null]),
            "workspace/configuration should mirror the requested item count",
        );
    }

    #[test]
    fn notifications_do_not_receive_responses() {
        let mut service = ClientRequestService;
        let notification = JsonRpcRequest::build("window/logMessage")
            .params(json!({"type": 3, "message": "ready"}))
            .finish();

        let response =
            block_on(service.call(notification)).expect("client notification should be handled");

        assert_eq!(
            response, None,
            "server-to-client notifications must not receive JSON-RPC responses",
        );
    }

    #[test]
    fn response_sink_routes_response_to_matching_waiter() {
        let pending = PendingResponses::default();
        let id = JsonRpcId::Number(7);
        let (sender, receiver) = oneshot::channel();
        pending
            .lock()
            .expect("pending response mutex should not be poisoned")
            .insert(id.clone(), sender);
        let mut sink = TowerResponseSink { pending };

        Pin::new(&mut sink)
            .start_send(JsonRpcResponse::from_ok(id, json!("ok")))
            .expect("response should be accepted");

        let response = block_on(receiver).expect("matching waiter should receive the response");
        let (_id, result) = response.into_parts();
        assert_eq!(
            result.expect("response should be successful"),
            json!("ok"),
            "response payload should be preserved",
        );
    }
}
