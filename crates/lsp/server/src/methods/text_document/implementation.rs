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
pub(crate) async fn implementation(
    ctx: DocumentMethodContext,
    params: GotoImplementationParams,
) -> Result<Option<GotoImplementationResponse>> {
    let position = params.text_document_position_params.position;
    let input = ctx.target_position(position)?;
    tracing::trace!("implementation request received");
    let result = ctx
        .engine_client
        .query(
            "goto_implementation",
            move |engine_client, request_context| async move {
                engine_client
                    .goto_implementation(request_context, input)
                    .await
            },
        )
        .await;
    let locations = ctx.finish_query(result)?;
    tracing::trace!(
        result_count = locations.len(),
        "implementation request answered"
    );

    Ok(Some(GotoDefinitionResponse::Array(locations)))
}
