use tower_lsp_server::{
    gen_lsp_types::{Range, SemanticTokens},
    jsonrpc::Result,
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
