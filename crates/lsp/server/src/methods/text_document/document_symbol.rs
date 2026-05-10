use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::methods::{MethodContext, internal_error, uri_to_path};

#[tracing::instrument(level = "trace", skip_all)]
pub(crate) async fn document_symbol(
    ctx: MethodContext,
    params: DocumentSymbolParams,
) -> Result<Option<DocumentSymbolResponse>> {
    let Some(path) = uri_to_path(&params.text_document.uri) else {
        return Ok(None);
    };
    tracing::trace!("document symbol request received");

    let symbols = ctx
        .engine_client
        .call(
            "document_symbol",
            move |client, request_context| async move {
                client.document_symbol(request_context, path).await
            },
        )
        .await
        .map_err(internal_error)?;
    tracing::trace!(
        result_count = symbols.len(),
        "document symbol request answered"
    );

    Ok(Some(DocumentSymbolResponse::Nested(symbols)))
}
