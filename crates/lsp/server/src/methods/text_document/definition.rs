use tower_lsp_server::{gen_lsp_types::*, jsonrpc::Result};

use crate::methods::DocumentMethodContext;

#[tracing::instrument(
    level = "trace", skip_all,
    fields(
        rg.position = ?params.text_document_position_params.position
    )
)]
pub(crate) async fn definition(
    ctx: DocumentMethodContext,
    params: DefinitionParams,
) -> Result<Option<DefinitionResponse>> {
    let position = params.text_document_position_params.position;
    let input = ctx.global_position(position)?;
    tracing::trace!("definition request received");
    let result = ctx
        .engine_client
        .query(
            "goto_definition",
            move |engine_client, request_context| async move {
                engine_client.goto_definition(request_context, input).await
            },
        )
        .await;
    let locations = ctx.finish_global_operation(result)?;
    tracing::trace!(
        result_count = locations.len(),
        "definition request answered"
    );

    Ok(Some(DefinitionResponse::Definition(
        Definition::LocationList(locations),
    )))
}
