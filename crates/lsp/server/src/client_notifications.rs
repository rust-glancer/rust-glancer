use std::path::{Path, PathBuf};

use tower_lsp_server::ls_types::{LSPAny, LSPObject, notification::Notification};

const ACTIVE_WORKSPACE_CHANGED_METHOD: &str = "rust-glancer/activeWorkspaceChanged";
const DEFERRED_INDEXING_FINISHED_METHOD: &str = "rust-glancer/deferredIndexingFinished";

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

/// Custom notification used by tooling that wants to distinguish structural readiness from the
/// background indexing work that completes shortly after it.
///
/// Editors can ignore this notification. `compare-lsp` uses it as a precise post-ready barrier so
/// its measured query latency does not include the first body-sensitive request materializing
/// deferred indexes on demand.
pub(crate) struct DeferredIndexingFinished;

impl Notification for DeferredIndexingFinished {
    type Params = LSPAny;

    const METHOD: &'static str = DEFERRED_INDEXING_FINISHED_METHOD;
}

impl DeferredIndexingFinished {
    pub(crate) fn params(root: &Path) -> LSPAny {
        let mut params = LSPObject::new();
        params.insert(
            "root".to_string(),
            LSPAny::String(root.display().to_string()),
        );
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
                "root: workspace/project_a\nstate: indexing",
            ),
            (
                ActiveWorkspaceStatus {
                    root: PathBuf::from("workspace/project_b"),
                    state: ActiveWorkspaceState::Ready,
                    message: None,
                },
                "root: workspace/project_b\nstate: ready",
            ),
            (
                ActiveWorkspaceStatus {
                    root: PathBuf::from("workspace/project_c"),
                    state: ActiveWorkspaceState::Failed,
                    message: Some("engine process exited unexpectedly".to_string()),
                },
                "message: engine process exited unexpectedly\nroot: workspace/project_c\nstate: failed",
            ),
        ];

        for (status, expected) in cases {
            let actual = render_params(ActiveWorkspaceChanged::params(&status));
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn deferred_indexing_finished_params_render_root() {
        let actual = render_params(DeferredIndexingFinished::params(Path::new(
            "workspace/project_a",
        )));
        assert_eq!(actual, "root: workspace/project_a");
    }

    fn render_params(params: LSPAny) -> String {
        let LSPAny::Object(params) = params else {
            panic!("active workspace notification params should be an object");
        };

        let mut entries = params
            .into_iter()
            .map(|(key, value)| {
                let LSPAny::String(value) = value else {
                    panic!("active workspace notification field `{key}` should be a string");
                };
                format!("{key}: {value}")
            })
            .collect::<Vec<_>>();
        entries.sort();
        entries.join("\n")
    }
}
