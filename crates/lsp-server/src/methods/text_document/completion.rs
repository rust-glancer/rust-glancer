use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::{
    backend::ServerContext,
    methods::{internal_error, uri_to_path},
};

pub(crate) async fn completion(
    ctx: &ServerContext,
    params: CompletionParams,
) -> Result<Option<CompletionResponse>> {
    let Some(path) = uri_to_path(&params.text_document_position.text_document.uri) else {
        return Ok(None);
    };
    let position = params.text_document_position.position;
    let trigger = params
        .context
        .as_ref()
        .and_then(|context| context.trigger_character.as_deref());
    tracing::trace!(
        path = %path.display(),
        line = position.line,
        character = position.character,
        trigger,
        "completion request received"
    );
    let completions = ctx
        .engine
        .completion(path.clone(), position)
        .await
        .map_err(internal_error)?;
    tracing::trace!(
        path = %path.display(),
        line = position.line,
        character = position.character,
        trigger,
        result_count = completions.len(),
        "completion request answered"
    );

    Ok(Some(CompletionResponse::Array(completions)))
}
