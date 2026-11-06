use tower_lsp_server::jsonrpc::Result;

use crate::{
    engine_registry::EngineRegistry,
    ingress::{self, LifecycleEvent},
};

/// Remove the engine-registry route after earlier open/save work for this path finishes.
pub(crate) async fn did_close(registry: Result<&EngineRegistry>) {
    let Some(LifecycleEvent::Close { route, .. }) = ingress::lifecycle_event().await else {
        tracing::error!("didClose bypassed ordered LSP ingress");
        return;
    };

    // Registry ownership uses the normalized source selected at open time. Reusing that route also
    // handles editor spellings such as symlinks and Windows aliases without normalizing twice.
    if let Ok(registry) = registry
        && let Ok(source_path) = route.source_path()
    {
        registry.close_document(&source_path).await;
    }
}
