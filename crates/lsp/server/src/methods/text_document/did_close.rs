use tower_lsp_server::ls_types::*;

use crate::methods::{MethodContext, uri_to_path};

#[tracing::instrument(level = "trace", skip_all)]
pub(crate) async fn did_close(ctx: MethodContext, params: DidCloseTextDocumentParams) {
    let Some(path) = uri_to_path(&params.text_document.uri) else {
        return;
    };

    ctx.engine_client
        .notify(
            "did_close",
            move |engine_client, request_context| async move {
                engine_client.did_close(request_context, path).await
            },
        )
        .await;
}
