use std::path::PathBuf;

use tower_lsp_server::ls_types::{LSPAny, LSPObject, notification::Notification};

const ACTIVE_WORKSPACE_CHANGED_METHOD: &str = "rust-glancer/activeWorkspaceChanged";

/// Custom notification that lets the VS Code client show which workspace currently owns requests.
///
/// This is intentionally UI-only. The reported root can be a user-facing display root rather than
/// the exact engine root; routing remains server-owned.
pub(crate) struct ActiveWorkspaceChanged;

impl Notification for ActiveWorkspaceChanged {
    type Params = LSPAny;

    const METHOD: &'static str = ACTIVE_WORKSPACE_CHANGED_METHOD;
}

impl ActiveWorkspaceChanged {
    pub(crate) fn params(status: &ActiveWorkspaceStatus) -> LSPAny {
        let mut params = LSPObject::new();
        params.insert(
            "root".to_string(),
            LSPAny::String(status.root.display().to_string()),
        );
        params.insert(
            "state".to_string(),
            LSPAny::String(status.state.as_str().to_string()),
        );
        if let Some(message) = &status.message {
            params.insert("message".to_string(), LSPAny::String(message.clone()));
        }
        LSPAny::Object(params)
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
