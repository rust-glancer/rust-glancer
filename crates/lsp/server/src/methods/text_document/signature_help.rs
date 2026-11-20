use rg_lsp_proto::SignatureHelpClientCapabilities;
use tower_lsp_server::{
    gen_lsp_types::{SignatureHelp, SignatureHelpParams},
    jsonrpc::Result,
};

use crate::methods::DocumentMethodContext;

pub(crate) async fn signature_help(
    ctx: DocumentMethodContext,
    params: SignatureHelpParams,
    client_capabilities: SignatureHelpClientCapabilities,
) -> Result<Option<SignatureHelp>> {
    let input = ctx.position(params.text_document_position_params.position)?;
    let result = ctx
        .engine_client
        .query(
            "signature_help",
            move |engine_client, request_context| async move {
                engine_client
                    .signature_help(request_context, input, client_capabilities)
                    .await
            },
        )
        .await;
    ctx.finish_semantic_query(result)
}
