use tower_lsp_server::{
    jsonrpc::Result,
    ls_types::{request::*, *},
};

use crate::methods::{MethodContext, internal_error, uri_to_path};

#[tracing::instrument(
    level = "trace", skip_all,
    fields(
        path = %*params.text_document_position_params.text_document.uri,
        position = ?params.text_document_position_params.position
    )
)]
pub(crate) async fn type_definition(
    ctx: MethodContext<'_>,
    params: GotoTypeDefinitionParams,
) -> Result<Option<GotoTypeDefinitionResponse>> {
    let Some(path) = uri_to_path(&params.text_document_position_params.text_document.uri) else {
        return Ok(None);
    };
    let position = params.text_document_position_params.position;
    tracing::trace!("type definition request received");
    let locations = ctx
        .engine_client
        .call(
            "goto_type_definition",
            move |client, request_context| async move {
                client
                    .goto_type_definition(request_context, path, position)
                    .await
            },
        )
        .await
        .map_err(internal_error)?;
    tracing::trace!(
        result_count = locations.len(),
        "type definition request answered"
    );

    Ok(Some(GotoDefinitionResponse::Array(locations)))
}
