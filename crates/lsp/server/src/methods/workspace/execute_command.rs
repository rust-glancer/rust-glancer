use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::{commands, methods::MethodContext, methods::internal_error};

#[tracing::instrument(
    level = "trace", skip_all,
    fields(command = %params.command)
)]
pub(crate) async fn execute_command(
    ctx: MethodContext,
    params: ExecuteCommandParams,
) -> Result<Option<LSPAny>> {
    let command = params.command;

    match command.as_str() {
        commands::REINDEX_WORKSPACE => {
            ctx.engine_client
                .call("reindex_workspace", |client, request_context| async move {
                    client.reindex_workspace(request_context).await
                })
                .await
                .map_err(internal_error)?;
            Ok(None)
        }
        command => Err(tower_lsp_server::jsonrpc::Error::invalid_params(format!(
            "unsupported rust-glancer command `{command}`",
        ))),
    }
}
