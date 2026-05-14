use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::methods::{MethodContext, internal_error, uri_to_path};

#[tracing::instrument(
    level = "trace", skip_all,
    fields(
        rg.position = ?params.text_document_position.position,
    )
)]
pub(crate) async fn completion(
    ctx: MethodContext,
    params: CompletionParams,
) -> Result<Option<CompletionResponse>> {
    let Some(path) = uri_to_path(&params.text_document_position.text_document.uri) else {
        return Ok(None);
    };
    let position = params.text_document_position.position;
    tracing::trace!("completion request received");
    let completions = ctx
        .engine_client
        .call(
            "completion",
            move |engine_client, request_context| async move {
                engine_client
                    .completion(request_context, path, position)
                    .await
            },
        )
        .await
        .map_err(internal_error)?;
    tracing::trace!(
        result_count = completions.len(),
        "completion request answered"
    );

    Ok(Some(CompletionResponse::Array(completions)))
}
