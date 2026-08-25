//! Persistent health through rust-analyzer's `experimental/serverStatus` extension.
//!
//! This flow is process-wide rather than tied to one indexing operation. Zed advertises
//! `experimental.serverStatusNotification` and uses `health` plus `message` for the Language
//! Servers menu. Its handler does not deserialize `quiescent`, so that field describes background
//! idleness for protocol completeness rather than driving Zed's indexing UI.

use tower_lsp_server::{
    Client as LspClient,
    ls_types::{ClientCapabilities, LSPAny, LSPObject, notification::Notification},
};

const SERVER_STATUS_CAPABILITY: &str = "serverStatusNotification";
const SERVER_STATUS_METHOD: &str = "experimental/serverStatus";

pub(super) fn is_supported(capabilities: &ClientCapabilities) -> bool {
    capabilities
        .experimental
        .as_ref()
        .and_then(|experimental| experimental.get(SERVER_STATUS_CAPABILITY))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

pub(super) async fn publish(lsp_client: &LspClient, status: &StatusSnapshot) {
    lsp_client
        .send_notification::<ServerStatus>(ServerStatus::params(status))
        .await;
}

struct ServerStatus;

impl Notification for ServerStatus {
    type Params = LSPAny;

    const METHOD: &'static str = SERVER_STATUS_METHOD;
}

impl ServerStatus {
    fn params(status: &StatusSnapshot) -> LSPAny {
        let mut params = LSPObject::new();
        params.insert(
            "health".to_string(),
            LSPAny::String(status.health.as_str().to_string()),
        );
        params.insert("quiescent".to_string(), LSPAny::Bool(status.quiescent));
        if let Some(message) = &status.message {
            params.insert("message".to_string(), LSPAny::String(message.clone()));
        }
        LSPAny::Object(params)
    }
}

/// Persistent health and background-idleness snapshot for one LSP server process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct StatusSnapshot {
    pub(super) health: Health,
    pub(super) quiescent: bool,
    pub(super) message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Health {
    Ok,
    Warning,
    Error,
}

impl Health {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_status_params_follow_rust_analyzer_shape() {
        let params = ServerStatus::params(&StatusSnapshot {
            health: Health::Warning,
            quiescent: false,
            message: Some("one workspace failed".to_string()),
        });

        assert_eq!(
            params,
            serde_json::json!({
                "health": "warning",
                "quiescent": false,
                "message": "one workspace failed",
            })
        );
    }
}
