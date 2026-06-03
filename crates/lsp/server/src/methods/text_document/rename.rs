use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::methods::{MethodContext, internal_error, uri_to_path};

#[tracing::instrument(
    level = "trace", skip_all,
    fields(rg.position = ?params.position)
)]
pub(crate) async fn prepare_rename(
    ctx: MethodContext,
    params: TextDocumentPositionParams,
) -> Result<Option<PrepareRenameResponse>> {
    let Some(path) = uri_to_path(&params.text_document.uri) else {
        return Ok(None);
    };
    let position = params.position;
    tracing::trace!("prepare rename request received");
    let response = ctx
        .engine_client
        .call(
            "prepare_rename",
            move |engine_client, request_context| async move {
                engine_client
                    .prepare_rename(request_context, path, position)
                    .await
            },
        )
        .await
        .map_err(internal_error)?;
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
    ctx: MethodContext,
    params: RenameParams,
) -> Result<Option<WorkspaceEdit>> {
    let Some(path) = uri_to_path(&params.text_document_position.text_document.uri) else {
        return Ok(None);
    };
    let position = params.text_document_position.position;
    let new_name = params.new_name;
    tracing::trace!("rename request received");
    let edit = ctx
        .engine_client
        .call("rename", move |engine_client, request_context| async move {
            engine_client
                .rename(request_context, path, position, new_name)
                .await
        })
        .await
        .map_err(internal_error)?;
    tracing::trace!(has_edit = edit.is_some(), "rename request answered");

    Ok(edit)
}
