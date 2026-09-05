//! Standard LSP work-done progress.
//!
//! This is the portable, operation-shaped status flow. Zed renders workspace indexing from this
//! protocol, while VS Code and other LSP clients can consume the same messages without knowing any
//! Rust Glancer extensions.
//!
//! Foreground indexing and deferred completion use separate server-owned handles. Their titles make
//! the queryable boundary explicit, while later phase/count signals update the deferred operation.
//! Engine services can also supply their own tokens for bounded operations such as Cargo
//! diagnostics; those begin/end messages are encoded here but do not change workspace lifecycle.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use rg_lsp_proto::{IndexingProgress, IndexingStage};
use tower_lsp_server::{
    Client as LspClient, NotCancellable, OngoingProgress, Unbounded,
    ls_types::{
        ClientCapabilities, NumberOrString, ProgressParams, ProgressParamsValue, WorkDoneProgress,
        WorkDoneProgressBegin, WorkDoneProgressEnd, notification::Progress,
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
        self.begin(lsp_client, root, foreground_indexing_title(root))
            .await;
    }

    pub(super) async fn begin_deferred(&mut self, lsp_client: &LspClient, root: &Path) {
        self.begin(lsp_client, root, deferred_indexing_title(root))
            .await;
    }

    async fn begin(&mut self, lsp_client: &LspClient, root: &Path, title: String) {
        if let Some(progress) = self.progress_by_root.remove(root) {
            progress.finish_with_message("Superseded").await;
        }

        let token = NumberOrString::String(format!(
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

    /// Update one active workspace operation with an editor-facing package count.
    pub(super) async fn report(&self, root: &Path, progress: IndexingProgress) {
        if let Some(operation) = self.progress_by_root.get(root) {
            operation.report(indexing_progress_message(progress)).await;
        }
    }
}

type WorkspaceProgress = OngoingProgress<Unbounded, NotCancellable>;

/// Begin an engine-owned progress operation whose token already crossed the process boundary.
pub(crate) async fn begin_engine_progress(
    lsp_client: &LspClient,
    token: NumberOrString,
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
        .send_notification::<Progress>(ProgressParams {
            token,
            value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(WorkDoneProgressBegin {
                title,
                cancellable: Some(false),
                message,
                percentage: None,
            })),
        })
        .await;
}

pub(crate) async fn end_engine_progress(
    lsp_client: &LspClient,
    token: NumberOrString,
    message: Option<String>,
) {
    lsp_client
        .send_notification::<Progress>(ProgressParams {
            token,
            value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(WorkDoneProgressEnd {
                message,
            })),
        })
        .await;
}

fn workspace_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| root.display().to_string())
}

fn foreground_indexing_title(root: &Path) -> String {
    format!("Indexing {}", workspace_name(root))
}

fn deferred_indexing_title(root: &Path) -> String {
    format!("{} ready · background", workspace_name(root))
}

fn indexing_progress_message(progress: IndexingProgress) -> String {
    let stage = match progress.stage {
        IndexingStage::LoweringBodies => "Lowering",
        IndexingStage::ResolvingBodies => "Resolving",
    };
    format!(
        "{stage} · {}/{}",
        progress.completed_packages, progress.total_packages
    )
}
