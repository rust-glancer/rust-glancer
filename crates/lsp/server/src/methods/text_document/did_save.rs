use std::path::PathBuf;

use tower_lsp_server::ls_types::*;

use crate::methods::MethodContext;

#[tracing::instrument(
    level = "trace", skip_all,
    fields(rg.has_text = params.text.is_some())
)]
pub(crate) async fn did_save(ctx: MethodContext, path: PathBuf, params: DidSaveTextDocumentParams) {
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
