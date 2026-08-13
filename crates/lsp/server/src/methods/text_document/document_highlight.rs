use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::methods::DocumentMethodContext;

#[tracing::instrument(
    level = "trace", skip_all,
    fields(
        rg.position = ?params.text_document_position_params.position
    )
)]
pub(crate) async fn document_highlight(
    ctx: DocumentMethodContext,
    params: DocumentHighlightParams,
) -> Result<Option<Vec<DocumentHighlight>>> {
    let position = params.text_document_position_params.position;
    let input = ctx.target_position(position)?;
    tracing::trace!("document highlight request received");
    let result = ctx
        .engine_client
        .query(
            "document_highlight",
            move |engine_client, request_context| async move {
                engine_client
                    .document_highlight(request_context, input)
                    .await
            },
        )
        .await;
    let highlights = ctx.finish_query(result)?;
    tracing::trace!(
        result_count = highlights.len(),
        "document highlight request answered"
    );

    Ok(Some(highlights))
}
