use tower_lsp_server::{
    jsonrpc::Result,
    ls_types::{request::*, *},
};

use crate::methods::DocumentMethodContext;

#[tracing::instrument(
    level = "trace", skip_all,
    fields(
        rg.position = ?params.text_document_position_params.position
    )
)]
pub(crate) async fn type_definition(
    ctx: DocumentMethodContext,
    params: GotoTypeDefinitionParams,
) -> Result<Option<GotoTypeDefinitionResponse>> {
    let position = params.text_document_position_params.position;
    let input = ctx.global_position(position)?;
    tracing::trace!("type definition request received");
    let result = ctx
        .engine_client
        .query(
            "goto_type_definition",
            move |engine_client, request_context| async move {
                engine_client
                    .goto_type_definition(request_context, input)
                    .await
            },
        )
        .await;
    let locations = ctx.finish_global_document_read(result)?;
    tracing::trace!(
        result_count = locations.len(),
        "type definition request answered"
    );

    Ok(Some(GotoDefinitionResponse::Array(locations)))
}
