use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::{
    backend::ServerContext,
    methods::{internal_error, uri_to_path},
};

pub(crate) async fn document_symbol(
    ctx: &ServerContext,
    params: DocumentSymbolParams,
) -> Result<Option<DocumentSymbolResponse>> {
    let Some(path) = uri_to_path(&params.text_document.uri) else {
        return Ok(None);
    };
    tracing::trace!(
        path = %path.display(),
        "document symbol request received"
    );

    let symbols = ctx
        .engine
        .document_symbol(path.clone())
        .await
        .map_err(internal_error)?;
    tracing::trace!(
        path = %path.display(),
        result_count = symbols.len(),
        "document symbol request answered"
    );

    Ok(Some(DocumentSymbolResponse::Nested(symbols)))
}
