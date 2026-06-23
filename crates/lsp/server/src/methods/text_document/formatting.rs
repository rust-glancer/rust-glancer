use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::methods::{MethodContext, internal_error, uri_to_path};

#[tracing::instrument(level = "trace", skip_all)]
pub(crate) async fn formatting(
    ctx: MethodContext,
    params: DocumentFormattingParams,
) -> Result<Option<Vec<TextEdit>>> {
    let Some(path) = uri_to_path(&params.text_document.uri) else {
        return Ok(None);
    };
    tracing::trace!("formatting request received");

    let edits = ctx
        .engine_client
        .call(
            "formatting",
            move |engine_client, request_context| async move {
                engine_client.formatting(request_context, path).await
            },
        )
        .await
        .map_err(internal_error)?;
    tracing::trace!(
        result_count = edits.as_ref().map(Vec::len),
        "formatting request answered"
    );

    Ok(edits)
}
