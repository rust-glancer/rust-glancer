//! Standard LSP work-done progress.
//!
//! This is the portable, operation-shaped status flow. Zed renders workspace indexing from this
//! protocol, while VS Code and other LSP clients can consume the same messages without knowing any
//! Rust Glancer extensions.
//!
//! Engine services can also supply their own tokens for bounded operations such as Cargo
//! diagnostics; those begin/end messages are encoded here but do not change workspace lifecycle.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use tower_lsp_server::{
    Client as LspClient, NotCancellable, OngoingProgress, Unbounded,
    gen_lsp_types::{
        ClientCapabilities, ProgressNotification, ProgressParams, ProgressToken,
        WorkDoneProgressBegin, WorkDoneProgressEnd,
    },
};

const INDEXING_PROGRESS_TOKEN_PREFIX: &str = "rust-glancer/indexing";

pub(super) fn is_supported(capabilities: &ClientCapabilities) -> bool {
    capabilities
        .window
        .as_ref()
        .and_then(|window| window.work_done_progress)
        .unwrap_or(false)
}

/// Active workspace indexing operations keyed by the root shown to the user.
#[derive(Debug, Default)]
pub(super) struct WorkspaceProgressState {
    progress_by_root: BTreeMap<PathBuf, WorkspaceProgress>,
    next_sequence: u64,
}

impl WorkspaceProgressState {
    pub(super) async fn begin_foreground(&mut self, lsp_client: &LspClient, root: &Path) {
        self.begin(lsp_client, root, Self::foreground_indexing_title(root))
            .await;
    }

    async fn begin(&mut self, lsp_client: &LspClient, root: &Path, title: String) {
        if let Some(progress) = self.progress_by_root.remove(root) {
            progress.finish_with_message("Superseded").await;
        }

        let token = ProgressToken::String(format!(
            "{INDEXING_PROGRESS_TOKEN_PREFIX}/{}",
            self.next_sequence
        ));
        self.next_sequence += 1;

        match lsp_client.create_work_done_progress(token.clone()).await {
            Ok(()) => {
                let progress = lsp_client.progress(token, title).begin().await;
                self.progress_by_root.insert(root.to_path_buf(), progress);
            }
            Err(error) => {
                tracing::debug!(
                    root = %root.display(),
                    error = %error,
                    "failed to create workspace indexing progress"
                );
            }
        }
    }

    pub(super) async fn finish(&mut self, root: &Path, message: &'static str) {
        if let Some(progress) = self.progress_by_root.remove(root) {
            progress.finish_with_message(message).await;
        }
    }

    fn foreground_indexing_title(root: &Path) -> String {
        format!("Indexing {}", Self::workspace_name(root))
    }

    fn workspace_name(root: &Path) -> String {
        root.file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| root.display().to_string())
    }
}

type WorkspaceProgress = OngoingProgress<Unbounded, NotCancellable>;

/// Begin an engine-owned progress operation whose token already crossed the process boundary.
pub(crate) async fn begin_engine_progress(
    lsp_client: &LspClient,
    token: ProgressToken,
    title: String,
    message: Option<String>,
) {
    if let Err(error) = lsp_client.create_work_done_progress(token.clone()).await {
        tracing::debug!(
            error = %error,
            "failed to create service progress token"
        );
        return;
    }

    lsp_client
        .send_notification::<ProgressNotification>(ProgressParams {
            token,
            value: serde_json::to_value(WorkDoneProgressBegin {
                title,
                cancellable: Some(false),
                message,
                percentage: None,
            })
            .expect("work-done progress should serialize"),
        })
        .await;
}

pub(crate) async fn end_engine_progress(
    lsp_client: &LspClient,
    token: ProgressToken,
    message: Option<String>,
) {
    lsp_client
        .send_notification::<ProgressNotification>(ProgressParams {
            token,
            value: serde_json::to_value(WorkDoneProgressEnd { message })
                .expect("work-done progress should serialize"),
        })
        .await;
}
