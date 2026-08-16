use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::methods::DocumentMethodContext;

#[tracing::instrument(level = "trace", skip_all)]
pub(crate) async fn formatting(
    ctx: DocumentMethodContext,
    _params: DocumentFormattingParams,
) -> Result<Option<Vec<TextEdit>>> {
    let document = ctx.target_document()?;
    tracing::trace!("formatting request received");

    let result = ctx
        .engine_client
        .query(
            "formatting",
            move |engine_client, request_context| async move {
                engine_client.formatting(request_context, document).await
            },
        )
        .await;
    let edits = ctx.finish_target_query(result)?;
    tracing::trace!(
        result_count = edits.as_ref().map(Vec::len),
        "formatting request answered"
    );

    Ok(edits)
}
