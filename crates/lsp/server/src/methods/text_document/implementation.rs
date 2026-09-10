use tower_lsp_server::{gen_lsp_types::*, jsonrpc::Result};

use crate::methods::DocumentMethodContext;

#[tracing::instrument(
    level = "trace", skip_all,
    fields(
        rg.position = ?params.text_document_position_params.position
    )
)]
pub(crate) async fn implementation(
    ctx: DocumentMethodContext,
    params: ImplementationParams,
) -> Result<Option<ImplementationResponse>> {
    let position = params.text_document_position_params.position;
    let input = ctx.global_position(position)?;
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
    let locations = ctx.finish_global_operation(result)?;
    tracing::trace!(
        result_count = locations.len(),
        "implementation request answered"
    );

    Ok(Some(ImplementationResponse::Definition(
        Definition::LocationList(locations),
    )))
}
