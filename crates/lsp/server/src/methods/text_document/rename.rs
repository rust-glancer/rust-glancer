use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::methods::DocumentMethodContext;

#[tracing::instrument(
    level = "trace", skip_all,
    fields(rg.position = ?params.position)
)]
pub(crate) async fn prepare_rename(
    ctx: DocumentMethodContext,
    params: TextDocumentPositionParams,
) -> Result<Option<PrepareRenameResponse>> {
    let position = params.position;
    let input = ctx.target_position(position)?;
    tracing::trace!("prepare rename request received");
    let result = ctx
        .engine_client
        .query(
            "prepare_rename",
            move |engine_client, request_context| async move {
                engine_client.prepare_rename(request_context, input).await
            },
        )
        .await;
    let response = ctx.finish_query(result)?;
    tracing::trace!(
        has_result = response.is_some(),
        "prepare rename request answered"
    );

    Ok(response)
}

#[tracing::instrument(
    level = "trace", skip_all,
    fields(
        rg.position = ?params.text_document_position.position,
        rg.new_name = %params.new_name
    )
)]
pub(crate) async fn rename(
    ctx: DocumentMethodContext,
    params: RenameParams,
) -> Result<Option<WorkspaceEdit>> {
    let position = params.text_document_position.position;
    let input = ctx.target_position(position)?;
    let new_name = params.new_name;
    tracing::trace!("rename request received");
    let result = ctx
        .engine_client
        .query("rename", move |engine_client, request_context| async move {
            engine_client.rename(request_context, input, new_name).await
        })
        .await;
    let edit = ctx.finish_query(result)?;
    tracing::trace!(has_edit = edit.is_some(), "rename request answered");

    Ok(edit)
}
