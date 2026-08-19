use tower_lsp_server::jsonrpc::Result;

use crate::{
    engine_registry::EngineRegistry,
    ingress::{self, LifecycleEvent},
};

/// Remove the engine-registry route and background-indexing priority for a closed session.
///
/// Earlier open/save work for the same path has finished before `lifecycle_event` returns. The
/// engine receives only a scheduling hint; synchronized text and session identity remain owned by
/// editor ingress.
pub(crate) async fn did_close(registry: Result<&EngineRegistry>) {
    let Some(LifecycleEvent::Close { path, route }) = ingress::lifecycle_event().await else {
        tracing::error!("didClose bypassed ordered LSP ingress");
        return;
    };

    // Remove the old route before a later reopen for this path can store its new route.
    if let Ok(registry) = registry {
        registry.close_document(&path).await;
    }
    if let Ok(engine_client) = route.engine_client() {
        engine_client
            .notify(
                "set_deferred_indexing_priority",
                move |engine_client, request_context| async move {
                    engine_client
                        .set_deferred_indexing_priority(request_context, path, false)
                        .await
                },
            )
            .await;
    }
}
