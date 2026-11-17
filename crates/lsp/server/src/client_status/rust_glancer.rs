//! Rust Glancer's private status notifications.
//!
//! The VS Code extension uses active-workspace events to render its status bar.

use std::path::PathBuf;

use rg_lsp_proto::path_for_editor;
use tower_lsp_server::{
    Client as LspClient,
    gen_lsp_types::{LspAny, LspNotificationMethod, LspObject, MessageDirection, Notification},
};

const ACTIVE_WORKSPACE_CHANGED_METHOD: &str = "rust-glancer/activeWorkspaceChanged";

pub(super) async fn active_workspace_changed(
    lsp_client: &LspClient,
    status: &ActiveWorkspaceStatus,
) {
    lsp_client
        .send_notification::<ActiveWorkspaceChanged>(ActiveWorkspaceChanged::params(status))
        .await;
}

struct ActiveWorkspaceChanged;

impl Notification for ActiveWorkspaceChanged {
    type Params = LspAny;

    const METHOD: LspNotificationMethod<'static> =
        LspNotificationMethod::Custom(ACTIVE_WORKSPACE_CHANGED_METHOD);
    const MESSAGE_DIRECTION: MessageDirection = MessageDirection::ServerToClient;
}

impl ActiveWorkspaceChanged {
    fn params(status: &ActiveWorkspaceStatus) -> LspAny {
        let mut params = LspObject::new();
        let root = path_for_editor(&status.root).unwrap_or_else(|_| status.root.clone());
        params.insert(
            "root".to_string(),
            LspAny::String(root.display().to_string()),
        );
        params.insert(
            "state".to_string(),
            LspAny::String(status.state.as_str().to_string()),
        );
        if let Some(message) = &status.message {
            params.insert("message".to_string(), LspAny::String(message.clone()));
        }
        LspAny::Object(params)
    }
}

/// Client-facing snapshot of the workspace currently selected by document routing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveWorkspaceStatus {
    pub(crate) root: PathBuf,
    pub(crate) state: ActiveWorkspaceState,
    pub(crate) message: Option<String>,
}

/// Small lifecycle vocabulary rendered by the VS Code status bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActiveWorkspaceState {
    Indexing,
    Ready,
    Failed,
}

impl ActiveWorkspaceState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Indexing => "indexing",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_workspace_changed_params_render_state_and_optional_message() {
        let cases = [
            (
                ActiveWorkspaceStatus {
                    root: PathBuf::from("workspace/project_a"),
                    state: ActiveWorkspaceState::Indexing,
                    message: None,
                },
                serde_json::json!({
                    "root": "workspace/project_a",
                    "state": "indexing",
                }),
            ),
            (
                ActiveWorkspaceStatus {
                    root: PathBuf::from("workspace/project_b"),
                    state: ActiveWorkspaceState::Ready,
                    message: None,
                },
                serde_json::json!({
                    "root": "workspace/project_b",
                    "state": "ready",
                }),
            ),
            (
                ActiveWorkspaceStatus {
                    root: PathBuf::from("workspace/project_c"),
                    state: ActiveWorkspaceState::Failed,
                    message: Some("engine process exited unexpectedly".to_string()),
                },
                serde_json::json!({
                    "root": "workspace/project_c",
                    "state": "failed",
                    "message": "engine process exited unexpectedly",
                }),
            ),
        ];

        for (status, expected) in cases {
            assert_eq!(ActiveWorkspaceChanged::params(&status), expected);
        }
    }
}
