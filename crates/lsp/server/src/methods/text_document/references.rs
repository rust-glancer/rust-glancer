use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::methods::DocumentMethodContext;

#[tracing::instrument(
    level = "trace", skip_all,
    fields(
        rg.position = ?params.text_document_position.position,
        rg.include_declaration = params.context.include_declaration
    )
)]
pub(crate) async fn references(
    ctx: DocumentMethodContext,
    params: ReferenceParams,
) -> Result<Option<Vec<Location>>> {
    let position = params.text_document_position.position;
    let input = ctx.target_position(position)?;
    let include_declaration = params.context.include_declaration;
    tracing::trace!("references request received");
    let result = ctx
        .engine_client
        .query(
            "references",
            move |engine_client, request_context| async move {
                engine_client
                    .references(request_context, input, include_declaration)
                    .await
            },
        )
        .await;
    let locations = ctx.finish_query(result)?;
    tracing::trace!(
        result_count = locations.len(),
        "references request answered"
    );

    Ok(Some(locations))
}
