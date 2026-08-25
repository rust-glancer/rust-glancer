//! Standard LSP work-done progress.
//!
//! This is the portable, operation-shaped status flow. Zed renders workspace indexing from this
//! protocol, while VS Code and other LSP clients can consume the same messages without knowing any
//! Rust Glancer extensions.
//!
//! Workspace indexing uses server-owned handles so a later phase/count signal can update the same
//! operation. Engine services can also supply their own tokens for bounded operations such as Cargo
//! diagnostics; those begin/end messages are encoded here but do not change workspace lifecycle.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

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
// TODO: Forward coalesced phase/count snapshots through the stored handle once indexing reports
// useful totals.
#[derive(Debug, Default)]
pub(super) struct WorkspaceProgressState {
    progress_by_root: BTreeMap<PathBuf, WorkspaceProgress>,
    next_sequence: u64,
}

impl WorkspaceProgressState {
    pub(super) async fn begin(&mut self, lsp_client: &LspClient, root: &Path) {
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
                let progress = lsp_client
                    .progress(token, indexing_title(root))
                    .begin()
                    .await;
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

fn indexing_title(root: &Path) -> String {
    let workspace = root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| root.display().to_string());
    format!("Indexing {workspace}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexing_title_reserves_the_message_for_future_phase_details() {
        assert_eq!(
            indexing_title(Path::new("/workspace/project_a")),
            "Indexing project_a"
        );
    }
}
