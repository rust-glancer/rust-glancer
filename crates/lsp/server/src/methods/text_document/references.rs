use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::methods::{MethodContext, internal_error, uri_to_path};

#[tracing::instrument(
    level = "trace", skip_all,
    fields(
        position = ?params.text_document_position.position,
        include_declaration = params.context.include_declaration
    )
)]
pub(crate) async fn references(
    ctx: MethodContext,
    params: ReferenceParams,
) -> Result<Option<Vec<Location>>> {
    let Some(path) = uri_to_path(&params.text_document_position.text_document.uri) else {
        return Ok(None);
    };
    let position = params.text_document_position.position;
    let include_declaration = params.context.include_declaration;
    tracing::trace!("references request received");
    let locations = ctx
        .engine_client
        .call(
            "references",
            move |engine_client, request_context| async move {
                engine_client
                    .references(request_context, path, position, include_declaration)
                    .await
            },
        )
        .await
        .map_err(internal_error)?;
    tracing::trace!(
        result_count = locations.len(),
        "references request answered"
    );

    Ok(Some(locations))
}
