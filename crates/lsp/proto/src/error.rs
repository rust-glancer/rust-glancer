use serde::{Deserialize, Serialize};

/// Shallow engine-side failure transported over the LSP/engine RPC boundary.
///
/// The cache and analysis layers already carry detailed context in their display strings. For now
/// the protocol only needs a stable error envelope; we can split this into typed variants later if
/// callers need recovery decisions instead of user-facing messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct EngineError {
    message: String,
}

impl EngineError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl From<anyhow::Error> for EngineError {
    fn from(error: anyhow::Error) -> Self {
        Self::new(error.to_string())
    }
}
