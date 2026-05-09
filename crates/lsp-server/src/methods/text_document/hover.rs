use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::{
    backend::ServerContext,
    methods::{internal_error, uri_to_path},
};

pub(crate) async fn hover(ctx: &ServerContext, params: HoverParams) -> Result<Option<Hover>> {
    let Some(path) = uri_to_path(&params.text_document_position_params.text_document.uri) else {
        return Ok(None);
    };
    let position = params.text_document_position_params.position;
    tracing::trace!(
        path = %path.display(),
        line = position.line,
        character = position.character,
        "hover request received"
    );
    let hover = ctx
        .engine
        .hover(path.clone(), position)
        .await
        .map_err(internal_error)?;
    tracing::trace!(
        path = %path.display(),
        line = position.line,
        character = position.character,
        has_hover = hover.is_some(),
        "hover request answered"
    );

    Ok(hover)
}
