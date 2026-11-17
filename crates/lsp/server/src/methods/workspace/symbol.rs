use tower_lsp_server::{gen_lsp_types::*, jsonrpc::Result};

use crate::{engine_client::EngineClient, ingress, methods::query_response};

#[tracing::instrument(
    level = "trace", skip_all,
    fields(rg.query = %params.query)
)]
pub(crate) async fn symbol(
    engine_client: EngineClient,
    params: WorkspaceSymbolParams,
) -> Result<Option<WorkspaceSymbolResponse>> {
    let captured = ingress::workspace_request().ok_or_else(|| {
        query_response::internal_error(anyhow::anyhow!("workspace request bypassed editor ingress"))
    })?;
    let snapshot = captured
        .snapshot(&engine_client)
        .map_err(|unavailable| query_response::temporarily_unavailable(unavailable.reason()))?;
    let query = params.query;
    tracing::trace!("workspace symbol request received");
    let symbols = query_response::validate_workspace(
        engine_client
            .query(
                "workspace_symbol",
                move |engine_client, request_context| async move {
                    engine_client
                        .workspace_symbol(request_context, snapshot, query)
                        .await
                },
            )
            .await,
        &captured,
    )
    .map_err(query_response::into_lsp_error)?;
    tracing::trace!(
        result_count = symbols.len(),
        "workspace symbol request answered"
    );

    Ok(Some(WorkspaceSymbolResponse::WorkspaceSymbolList(symbols)))
}
