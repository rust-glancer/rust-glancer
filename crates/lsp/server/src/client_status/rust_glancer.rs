//! Rust Glancer's private status notifications.
//!
//! The VS Code extension uses the active-workspace and deferred-indexing events to render its
//! detailed status bar. `compare-lsp` also uses deferred-indexing completion as a precise barrier
//! before measuring body-sensitive requests. These notifications predate the portable progress and
//! rust-analyzer-compatible health flows, so they remain additive compatibility contracts.

use std::path::{Path, PathBuf};

use tower_lsp_server::{
    Client as LspClient,
    ls_types::{LSPAny, LSPObject, notification::Notification},
};

const ACTIVE_WORKSPACE_CHANGED_METHOD: &str = "rust-glancer/activeWorkspaceChanged";
const DEFERRED_INDEXING_STARTED_METHOD: &str = "rust-glancer/deferredIndexingStarted";
const DEFERRED_INDEXING_FINISHED_METHOD: &str = "rust-glancer/deferredIndexingFinished";

pub(super) async fn active_workspace_changed(
    lsp_client: &LspClient,
    status: &ActiveWorkspaceStatus,
) {
    lsp_client
        .send_notification::<ActiveWorkspaceChanged>(ActiveWorkspaceChanged::params(status))
        .await;
}

pub(super) async fn deferred_indexing_started(lsp_client: &LspClient, root: &Path) {
    lsp_client
        .send_notification::<DeferredIndexingStarted>(DeferredIndexingStarted::params(root))
        .await;
}

pub(super) async fn deferred_indexing_finished(lsp_client: &LspClient, root: &Path) {
    lsp_client
        .send_notification::<DeferredIndexingFinished>(DeferredIndexingFinished::params(root))
        .await;
}

struct ActiveWorkspaceChanged;

impl Notification for ActiveWorkspaceChanged {
    type Params = LSPAny;

    const METHOD: &'static str = ACTIVE_WORKSPACE_CHANGED_METHOD;
}

impl ActiveWorkspaceChanged {
    fn params(status: &ActiveWorkspaceStatus) -> LSPAny {
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

/// Marks the beginning of background work for an already-queryable project generation.
///
/// This event is separate from the foreground `indexing` workspace state. A watcher batch can be
/// an exact replay that publishes no generation and therefore starts no deferred work.
struct DeferredIndexingStarted;

impl Notification for DeferredIndexingStarted {
    type Params = LSPAny;

    const METHOD: &'static str = DEFERRED_INDEXING_STARTED_METHOD;
}

impl DeferredIndexingStarted {
    fn params(root: &Path) -> LSPAny {
        deferred_indexing_params(root)
    }
}

/// Marks completion of background work for the active saved project generation.
struct DeferredIndexingFinished;

impl Notification for DeferredIndexingFinished {
    type Params = LSPAny;

    const METHOD: &'static str = DEFERRED_INDEXING_FINISHED_METHOD;
}

impl DeferredIndexingFinished {
    fn params(root: &Path) -> LSPAny {
        deferred_indexing_params(root)
    }
}

fn deferred_indexing_params(root: &Path) -> LSPAny {
    let mut params = LSPObject::new();
    params.insert(
        "root".to_string(),
        LSPAny::String(root.display().to_string()),
    );
    LSPAny::Object(params)
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
    fn deferred_indexing_params_render_root() {
        let root = Path::new("workspace/project_a");
        for params in [
            DeferredIndexingStarted::params(root),
            DeferredIndexingFinished::params(root),
        ] {
            assert_eq!(render_params(params), "root: workspace/project_a");
        }
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
