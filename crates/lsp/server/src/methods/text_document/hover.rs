use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::methods::DocumentMethodContext;

#[tracing::instrument(
    level = "trace", skip_all,
    fields(
        rg.position = ?params.text_document_position_params.position
    )
)]
pub(crate) async fn hover(
    ctx: DocumentMethodContext,
    params: HoverParams,
) -> Result<Option<Hover>> {
    let position = params.text_document_position_params.position;
    let input = ctx.target_position(position)?;
    tracing::trace!("hover request received");
    let result = ctx
        .engine_client
        .query("hover", move |engine_client, request_context| async move {
            engine_client.hover(request_context, input).await
        })
        .await;
    let hover = ctx.finish_document_read(result)?;
    tracing::trace!(has_hover = hover.is_some(), "hover request answered");

    Ok(hover)
}
