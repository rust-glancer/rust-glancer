use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::methods::{MethodContext, internal_error, uri_to_path};

#[tracing::instrument(
    level = "trace", skip_all,
    fields(
        path = %*params.text_document_position_params.text_document.uri,
        position = ?params.text_document_position_params.position
    )
)]
pub(crate) async fn hover(ctx: MethodContext<'_>, params: HoverParams) -> Result<Option<Hover>> {
    let Some(path) = uri_to_path(&params.text_document_position_params.text_document.uri) else {
        return Ok(None);
    };
    let position = params.text_document_position_params.position;
    tracing::trace!("hover request received");
    let hover = ctx
        .engine_client
        .call("hover", move |client, request_context| async move {
            client.hover(request_context, path, position).await
        })
        .await
        .map_err(internal_error)?;
    tracing::trace!(has_hover = hover.is_some(), "hover request answered");

    Ok(hover)
}
