use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::{engine_client::EngineClient, methods::query_response};

#[tracing::instrument(
    level = "trace", skip_all,
    fields(rg.query = %params.query)
)]
pub(crate) async fn symbol(
    engine_client: EngineClient,
    params: WorkspaceSymbolParams,
) -> Result<Option<WorkspaceSymbolResponse>> {
    let query = params.query;
    tracing::trace!("workspace symbol request received");
    let symbols = query_response::validate_workspace(
        engine_client
            .query(
                "workspace_symbol",
                move |engine_client, request_context| async move {
                    engine_client.workspace_symbol(request_context, query).await
                },
            )
            .await,
    )
    .map_err(query_response::into_lsp_error)?;
    tracing::trace!(
        result_count = symbols.len(),
        "workspace symbol request answered"
    );

    Ok(Some(WorkspaceSymbolResponse::Nested(symbols)))
}
