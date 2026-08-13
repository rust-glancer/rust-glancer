use tower_lsp_server::jsonrpc::Result;

use crate::{
    engine_registry::EngineRegistry,
    ingress::{self, LifecycleEvent},
};

/// Remove the engine-registry route for a session already marked closed in editor state.
///
/// Earlier open/save work for the same path has finished before `lifecycle_event` returns. The
/// engine itself stores no open-document lifecycle state, so it needs no close request.
pub(crate) async fn did_close(registry: Result<&EngineRegistry>) {
    let Some(LifecycleEvent::Close { path }) = ingress::lifecycle_event().await else {
        tracing::error!("didClose bypassed ordered LSP ingress");
        return;
    };

    // Remove the old route before a later reopen for this path can store its new route.
    if let Ok(registry) = registry {
        registry.close_document(&path).await;
    }
}
