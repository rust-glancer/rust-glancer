use tower_lsp_server::ls_types::*;

use crate::{backend::ServerContext, methods::uri_to_path};

pub(crate) async fn did_save(ctx: &ServerContext, params: DidSaveTextDocumentParams) {
    let Some(path) = uri_to_path(&params.text_document.uri) else {
        return;
    };

    ctx.engine.did_save(path, params.text).await;
}
