use tower_lsp_server::{
    jsonrpc::Result,
    ls_types::{Range, SemanticTokens},
};

use crate::methods::DocumentMethodContext;

pub(crate) async fn semantic_tokens(
    ctx: DocumentMethodContext,
    range: Option<Range>,
) -> Result<SemanticTokens> {
    let document = ctx.target_document()?;
    let result = ctx
        .engine_client
        .query(
            "semantic_tokens",
            move |engine, request_context| async move {
                engine
                    .semantic_tokens(request_context, document, range)
                    .await
            },
        )
        .await;
    ctx.finish_target_query(result)
}
