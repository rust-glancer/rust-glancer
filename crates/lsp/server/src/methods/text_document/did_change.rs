use tower_lsp_server::ls_types::*;

use crate::methods::{MethodContext, uri_to_path};

#[tracing::instrument(
    level = "trace", skip_all,
    fields(
        version = params.text_document.version,
        content_change_count = params.content_changes.len(),
        has_full_text = params.content_changes.last().is_some_and(|change| change.range.is_none())
    )
)]
pub(crate) async fn did_change(ctx: MethodContext, params: DidChangeTextDocumentParams) {
    let Some(path) = uri_to_path(&params.text_document.uri) else {
        return;
    };
    let full_text = params
        .content_changes
        .last()
        .and_then(|change| change.range.is_none().then_some(change.text.clone()));
    let version = Some(params.text_document.version);
    let content_change_count = params.content_changes.len();
    ctx.engine_client
        .notify("did_change", move |client, request_context| async move {
            client
                .did_change(
                    request_context,
                    path,
                    version,
                    full_text,
                    content_change_count,
                )
                .await
        })
        .await;
}
