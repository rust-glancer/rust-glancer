use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::{
    backend::ServerContext,
    methods::{internal_error, uri_to_path},
};

pub(crate) async fn inlay_hint(
    ctx: &ServerContext,
    params: InlayHintParams,
) -> Result<Option<Vec<InlayHint>>> {
    let Some(path) = uri_to_path(&params.text_document.uri) else {
        return Ok(None);
    };
    tracing::trace!(
        path = %path.display(),
        start_line = params.range.start.line,
        start_character = params.range.start.character,
        end_line = params.range.end.line,
        end_character = params.range.end.character,
        "inlay hint request received"
    );
    let hints = ctx
        .engine
        .inlay_hint(path.clone(), params.range)
        .await
        .map_err(internal_error)?;
    tracing::trace!(
        path = %path.display(),
        start_line = params.range.start.line,
        start_character = params.range.start.character,
        end_line = params.range.end.line,
        end_character = params.range.end.character,
        result_count = hints.len(),
        "inlay hint request answered"
    );

    Ok(Some(hints))
}
