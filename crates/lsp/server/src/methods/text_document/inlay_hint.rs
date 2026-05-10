use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::methods::{MethodContext, internal_error, uri_to_path};

#[tracing::instrument(
    level = "trace", skip_all,
    fields(path = %*params.text_document.uri, range = ?params.range)
)]
pub(crate) async fn inlay_hint(
    ctx: MethodContext<'_>,
    params: InlayHintParams,
) -> Result<Option<Vec<InlayHint>>> {
    let Some(path) = uri_to_path(&params.text_document.uri) else {
        return Ok(None);
    };
    let range = params.range;
    tracing::trace!("inlay hint request received");
    let hints = ctx
        .engine_client
        .call("inlay_hint", move |client, request_context| async move {
            client.inlay_hint(request_context, path, range).await
        })
        .await
        .map_err(internal_error)?;
    tracing::trace!(result_count = hints.len(), "inlay hint request answered");

    Ok(Some(hints))
}
