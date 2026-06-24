//! LSP client transport used by the comparison harness.
//!
//! The harness drives external LSP server processes as a client. Transport-specific framing and
//! routing stay behind this small async boundary so the server lifecycle code can focus on
//! process management and fixture setup.

mod tower_transport;

use serde_json::Value;

pub(crate) use self::tower_transport::TowerLspTransport;

/// Outcome of one outbound JSON-RPC request.
///
/// Keeping protocol errors separate from timeouts and transport failures lets the report explain
/// whether the server rejected a request or the conversation itself broke.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RequestOutcome {
    Success(Value),
    Error(RpcError),
    Timeout,
    TransportFailure { message: String },
}

/// JSON-RPC error object returned by a server for a request it handled.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RpcError {
    pub(crate) code: i64,
    pub(crate) message: String,
    pub(crate) data: Option<Value>,
}
