use rg_lsp_proto::FoldingClientCapabilities;
use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::methods::DocumentMethodContext;

#[tracing::instrument(level = "trace", skip_all)]
pub(crate) async fn folding_range(
    ctx: DocumentMethodContext,
    _params: FoldingRangeParams,
    client_capabilities: FoldingClientCapabilities,
) -> Result<Option<Vec<FoldingRange>>> {
    let document = ctx.target_document()?;
    tracing::trace!("folding range request received");

    let result = ctx
        .engine_client
        .query(
            "folding_range",
            move |engine_client, request_context| async move {
                engine_client
                    .folding_range(request_context, document, client_capabilities)
                    .await
            },
        )
        .await;
    let ranges = ctx.finish_target_query(result)?;
    tracing::trace!(
        result_count = ranges.len(),
        "folding range request answered"
    );

    Ok(Some(ranges))
}
