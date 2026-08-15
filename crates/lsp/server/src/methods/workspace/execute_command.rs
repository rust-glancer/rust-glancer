use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::{commands, engine_client::EngineClient, methods::internal_error};

#[tracing::instrument(
    level = "trace", skip_all,
    fields(rg.command = %params.command)
)]
pub(crate) async fn execute_command(
    engine_client: EngineClient,
    params: ExecuteCommandParams,
) -> Result<Option<LSPAny>> {
    let command = params.command;

    match command.as_str() {
        commands::REINDEX_WORKSPACE => {
            engine_client
                .call_project_update(
                    "reindex_workspace",
                    |engine_client, request_context| async move {
                        engine_client.reindex_workspace(request_context).await
                    },
                )
                .await
                .map_err(internal_error)?;
            Ok(None)
        }
        command => Err(tower_lsp_server::jsonrpc::Error::invalid_params(format!(
            "unsupported rust-glancer command `{command}`",
        ))),
    }
}
