use std::path::Path;

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
    pub(crate) fn params(root: &Path) -> LSPAny {
        let mut params = LSPObject::new();
        params.insert(
            "root".to_string(),
            LSPAny::String(root.display().to_string()),
        );
        LSPAny::Object(params)
    }
}
