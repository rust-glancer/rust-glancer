use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::methods::{MethodContext, internal_error, uri_to_path};

#[tracing::instrument(
    level = "trace", skip_all,
    fields(
        position = ?params.text_document_position_params.position
    )
)]
pub(crate) async fn document_highlight(
    ctx: MethodContext,
    params: DocumentHighlightParams,
) -> Result<Option<Vec<DocumentHighlight>>> {
    let Some(path) = uri_to_path(&params.text_document_position_params.text_document.uri) else {
        return Ok(None);
    };
    let position = params.text_document_position_params.position;
    tracing::trace!("document highlight request received");
    let highlights = ctx
        .engine_client
        .call(
            "document_highlight",
            move |engine_client, request_context| async move {
                engine_client
                    .document_highlight(request_context, path, position)
                    .await
            },
        )
        .await
        .map_err(internal_error)?;
    tracing::trace!(
        result_count = highlights.len(),
        "document highlight request answered"
    );

    Ok(Some(highlights))
}
