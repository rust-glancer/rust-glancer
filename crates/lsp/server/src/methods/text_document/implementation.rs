use tower_lsp_server::{
    jsonrpc::Result,
    ls_types::{request::*, *},
};

use crate::methods::{MethodContext, internal_error, uri_to_path};

#[tracing::instrument(
    level = "trace", skip_all,
    fields(
        position = ?params.text_document_position_params.position
    )
)]
pub(crate) async fn implementation(
    ctx: MethodContext,
    params: GotoImplementationParams,
) -> Result<Option<GotoImplementationResponse>> {
    let Some(path) = uri_to_path(&params.text_document_position_params.text_document.uri) else {
        return Ok(None);
    };
    let position = params.text_document_position_params.position;
    tracing::trace!("implementation request received");
    let locations = ctx
        .engine_client
        .call(
            "goto_implementation",
            move |engine_client, request_context| async move {
                engine_client
                    .goto_implementation(request_context, path, position)
                    .await
            },
        )
        .await
        .map_err(internal_error)?;
    tracing::trace!(
        result_count = locations.len(),
        "implementation request answered"
    );

    Ok(Some(GotoDefinitionResponse::Array(locations)))
}
