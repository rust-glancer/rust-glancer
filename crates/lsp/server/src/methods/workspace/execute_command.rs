use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::{backend::ServerContext, commands, methods::internal_error};

pub(crate) async fn execute_command(
    ctx: &ServerContext,
    params: ExecuteCommandParams,
) -> Result<Option<LSPAny>> {
    match params.command.as_str() {
        commands::REINDEX_WORKSPACE => {
            ctx.engine
                .reindex_workspace()
                .await
                .map_err(internal_error)?;
            Ok(None)
        }
        command => Err(tower_lsp_server::jsonrpc::Error::invalid_params(format!(
            "unsupported rust-glancer command `{command}`",
        ))),
    }
}
