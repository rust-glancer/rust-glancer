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

    // Registry ownership uses the normalized source selected at open time. Reusing that route also
    // handles editor spellings such as symlinks and Windows aliases without normalizing twice.
    if let Ok(registry) = registry
        && let Ok(source_path) = route.source_path()
    {
        registry.close_document(&source_path).await;
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
