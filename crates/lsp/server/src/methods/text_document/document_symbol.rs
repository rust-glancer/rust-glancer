use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::methods::DocumentMethodContext;

#[tracing::instrument(level = "trace", skip_all)]
pub(crate) async fn document_symbol(
    ctx: DocumentMethodContext,
    _params: DocumentSymbolParams,
) -> Result<Option<DocumentSymbolResponse>> {
    let document = ctx.target_document()?;
    tracing::trace!("document symbol request received");

    let result = ctx
        .engine_client
        .query(
            "document_symbol",
            move |engine_client, request_context| async move {
                engine_client
                    .document_symbol(request_context, document)
                    .await
            },
        )
        .await;
    let symbols = ctx.finish_document_read(result)?;
    tracing::trace!(
        result_count = symbols.len(),
        "document symbol request answered"
    );

    Ok(Some(DocumentSymbolResponse::Nested(symbols)))
}
