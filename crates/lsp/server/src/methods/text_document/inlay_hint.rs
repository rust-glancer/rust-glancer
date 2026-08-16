use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::methods::DocumentMethodContext;

#[tracing::instrument(
    level = "trace", skip_all,
    fields(rg.range = ?params.range)
)]
pub(crate) async fn inlay_hint(
    ctx: DocumentMethodContext,
    params: InlayHintParams,
) -> Result<Option<Vec<InlayHint>>> {
    let range = params.range;
    let input = ctx.target_range(range)?;
    tracing::trace!("inlay hint request received");
    let result = ctx
        .engine_client
        .query(
            "inlay_hint",
            move |engine_client, request_context| async move {
                engine_client.inlay_hint(request_context, input).await
            },
        )
        .await;
    let hints = ctx.finish_target_query(result)?;
    tracing::trace!(result_count = hints.len(), "inlay hint request answered");

    Ok(Some(hints))
}
