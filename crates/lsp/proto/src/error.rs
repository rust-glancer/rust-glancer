use serde::{Deserialize, Serialize};

/// Shallow engine-side failure transported over the LSP/engine RPC boundary.
///
/// The cache and analysis layers attach useful context through `anyhow`, but `anyhow::Error` only
/// displays the outermost context by default. The protocol keeps a stable string envelope and
/// preserves the full chain when converting from `anyhow` so the server does not lose the root
/// cause before it can log or surface the failure.
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
        Self::new(format!("{error:#}"))
    }
}

#[cfg(test)]
mod tests {
    use super::EngineError;

    #[test]
    fn anyhow_conversion_preserves_context_chain() {
        let error = anyhow::anyhow!("body data is unavailable")
            .context("while loading body IR")
            .context("while executing query");

        let message = EngineError::from(error).to_string();

        assert_eq!(
            message,
            "while executing query: while loading body IR: body data is unavailable",
        );
        assert!(
            !message.contains('\n'),
            "alternate anyhow display should keep context chains on one line",
        );
    }
}
