use tower_lsp_server::jsonrpc::Result;

use crate::{
    engine_registry::EngineRegistry,
    ingress::{self, LifecycleEvent},
};

/// Find the engine for a session whose text was recorded before this handler started.
///
/// Storing the result makes that engine route available to later document requests. The editor
/// session and its text remain available even when no engine can be found yet.
pub(crate) async fn did_open(registry: Result<&EngineRegistry>) {
    let Some(LifecycleEvent::Open { document, route }) = ingress::lifecycle_event().await else {
        tracing::error!("didOpen bypassed ordered LSP ingress");
        return;
    };
    let routed = match registry {
        Ok(registry) => registry.open_document(document.path()).await,
        Err(error) => Err(anyhow::anyhow!(error.message.into_owned())),
    };
    route.publish(routed);
    if route.engine_client().is_err() {
        tracing::debug!(
            path = %document.path().display(),
            session = document.session().get(),
            "retained opened editor document without a ready engine route"
        );
    }
}
