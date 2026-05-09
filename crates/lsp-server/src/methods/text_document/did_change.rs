use tower_lsp_server::ls_types::*;

use crate::{backend::ServerContext, methods::uri_to_path};

pub(crate) async fn did_change(ctx: &ServerContext, params: DidChangeTextDocumentParams) {
    let Some(path) = uri_to_path(&params.text_document.uri) else {
        return;
    };
    let full_text = params
        .content_changes
        .last()
        .and_then(|change| change.range.is_none().then_some(change.text.clone()));
    ctx.engine
        .did_change(
            path,
            Some(params.text_document.version),
            full_text,
            params.content_changes.len(),
        )
        .await;
}
