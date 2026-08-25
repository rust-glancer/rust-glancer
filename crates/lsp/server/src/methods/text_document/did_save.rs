use tower_lsp_server::{
    Client as LspClient,
    ls_types::{DidSaveTextDocumentParams, MessageType},
};

use crate::{
    ingress::{self, LifecycleEvent},
    inlay_refresher::InlayRefresher,
    methods,
};

/// Send the saved full text prepared before this async handler started to its engine.
///
/// Earlier open/save work for the same path has finished before `lifecycle_event` returns this
/// proposal, so the handler uses the route assigned to the matching open session.
pub(crate) async fn did_save(
    lsp_client: &LspClient,
    inlay_refresher: &InlayRefresher,
    params: DidSaveTextDocumentParams,
) {
    let Some(notified_path) = methods::uri_to_path(&params.text_document.uri) else {
        return;
    };
    let Some(LifecycleEvent::Save { proposal, route }) = ingress::lifecycle_event().await else {
        tracing::error!("didSave bypassed ordered LSP ingress");
        return;
    };
    let path = proposal.path().to_path_buf();
    if path != notified_path.as_path() {
        tracing::error!("didSave proposal does not match its LSP notification path");
        return;
    }

    let Ok(engine_client) = route.engine_client() else {
        tracing::debug!(
            path = %proposal.path().display(),
            session = proposal.target().session().get(),
            revision = proposal.target().revision().get(),
            client_version = ?proposal.client_version(),
            "retained editor save proposal without a ready engine route"
        );
        return;
    };

    let session = proposal.target().session().get();
    let revision = proposal.target().revision().get();
    let result = engine_client
        .call_project_update(
            "did_save",
            move |engine_client, request_context| async move {
                engine_client.did_save(request_context, proposal).await
            },
        )
        .await;
    let saved_project_generation = match result {
        Ok(saved_project_generation) => saved_project_generation,
        Err(error) => {
            let message = format!("failed to publish saved source: {error:#}");
            tracing::warn!(
                path = %path.display(),
                error = %format!("{error:#}"),
                "editor save failed"
            );
            lsp_client.log_message(MessageType::ERROR, message).await;
            return;
        }
    };
    inlay_refresher.document_saved();
    tracing::debug!(
        path = %path.display(),
        session,
        revision,
        saved_project_generation,
        "editor save publication acknowledged"
    );
}
