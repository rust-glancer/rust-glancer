use tower_lsp_server::ls_types::*;

use crate::methods::{MethodContext, uri_to_path};

#[tracing::instrument(
    level = "trace", skip_all,
    fields(rg.has_text = params.text.is_some())
)]
pub(crate) async fn did_save(ctx: MethodContext, params: DidSaveTextDocumentParams) {
    let Some(path) = uri_to_path(&params.text_document.uri) else {
        return;
    };

    ctx.engine_client
        .notify(
            "did_save",
            move |engine_client, request_context| async move {
                engine_client
                    .did_save(request_context, path, params.text)
                    .await
            },
        )
        .await;
}
