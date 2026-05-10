use tower_lsp_server::ls_types::*;

use crate::methods::{MethodContext, uri_to_path};

#[tracing::instrument(
    level = "trace", skip_all,
    fields(path = %*params.text_document.uri, version = params.text_document.version)
)]
pub(crate) async fn did_open(ctx: MethodContext<'_>, params: DidOpenTextDocumentParams) {
    let Some(path) = uri_to_path(&params.text_document.uri) else {
        return;
    };
    let version = Some(params.text_document.version);

    ctx.engine_client
        .notify("did_open", move |client, request_context| async move {
            client
                .did_open(request_context, path, version, params.text_document.text)
                .await
        })
        .await;
}
