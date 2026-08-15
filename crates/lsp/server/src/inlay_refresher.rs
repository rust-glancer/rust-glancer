//! Schedules inlay-hint refreshes after editor changes and successful saves.
//!
//! An edit schedules one refresh after the editor has been quiet briefly. Nearby edits replace
//! that pending refresh instead of starting one request each. A successful save cancels any
//! delayed edit refresh and asks the client to refresh immediately after the saved project accepts
//! the new source.
//!
//! This only tells the editor to request inlay hints again. Delaying, combining, or failing a
//! refresh does not change which document revision analysis may publish.

use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tower_lsp_server::Client as LspClient;

const INLAY_HINT_REFRESH_DEBOUNCE: Duration = Duration::from_millis(150);

/// Schedules inlay-hint refresh requests caused by editor changes and saves.
///
/// An edit is known at ordered ingress and does not need engine document mutation to justify a
/// refresh. A generation token coalesces nearby edits without retaining one task handle per open
/// document. Saves advance the same generation and request an immediate refresh.
#[derive(Clone, Debug, Default)]
pub(crate) struct InlayRefresher {
    state: Arc<InlayRefreshState>,
}

impl InlayRefresher {
    /// Attach the LSP client created while the server transport is assembled.
    pub(crate) fn bind(&self, client: LspClient) {
        self.state
            .client
            .set(client)
            .expect("inlay refresher client should only be bound once");
    }

    /// Coalesce nearby edits into one refresh after the editor has been quiet briefly.
    pub(crate) fn document_changed(&self) {
        let Some(client) = self.state.client.get().cloned() else {
            return;
        };
        let generation = self.state.next_generation();
        let state = Arc::clone(&self.state);

        tokio::spawn(async move {
            tokio::time::sleep(INLAY_HINT_REFRESH_DEBOUNCE).await;
            if state.generation.load(Ordering::Relaxed) == generation
                && let Err(error) = client.inlay_hint_refresh().await
            {
                tracing::debug!(
                    error = %error,
                    "failed to request inlay hint refresh after editor change"
                );
            }
        });
    }

    /// Cancel any delayed edit refresh and request inlay hints after save publication.
    pub(crate) fn document_saved(&self) {
        let Some(client) = self.state.client.get().cloned() else {
            return;
        };
        self.state.next_generation();

        tokio::spawn(async move {
            if let Err(error) = client.inlay_hint_refresh().await {
                tracing::debug!(
                    error = %error,
                    "failed to request inlay hint refresh after editor save"
                );
            }
        });
    }
}

#[derive(Debug, Default)]
struct InlayRefreshState {
    client: OnceLock<LspClient>,
    generation: AtomicU64,
}

impl InlayRefreshState {
    fn next_generation(&self) -> u64 {
        self.generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }
}
