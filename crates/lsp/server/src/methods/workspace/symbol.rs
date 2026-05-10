use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::{methods::MethodContext, methods::internal_error};

#[tracing::instrument(
    level = "trace", skip_all,
    fields(query = %params.query)
)]
pub(crate) async fn symbol(
    ctx: MethodContext,
    params: WorkspaceSymbolParams,
) -> Result<Option<WorkspaceSymbolResponse>> {
    let query = params.query;
    tracing::trace!("workspace symbol request received");
    let symbols = ctx
        .engine_client
        .call(
            "workspace_symbol",
            move |engine_client, request_context| async move {
                engine_client.workspace_symbol(request_context, query).await
            },
        )
        .await
        .map_err(internal_error)?;
    tracing::trace!(
        result_count = symbols.len(),
        "workspace symbol request answered"
    );

    Ok(Some(WorkspaceSymbolResponse::Nested(symbols)))
}
