use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::{backend::ServerContext, methods::internal_error};

pub(crate) async fn symbol(
    ctx: &ServerContext,
    params: WorkspaceSymbolParams,
) -> Result<Option<WorkspaceSymbolResponse>> {
    tracing::trace!(query = %params.query, "workspace symbol request received");
    let symbols = ctx
        .engine
        .workspace_symbol(params.query.clone())
        .await
        .map_err(internal_error)?;
    tracing::trace!(
        query = %params.query,
        result_count = symbols.len(),
        "workspace symbol request answered"
    );

    Ok(Some(WorkspaceSymbolResponse::Nested(symbols)))
}
