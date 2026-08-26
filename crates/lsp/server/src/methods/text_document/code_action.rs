//! LSP publication policy for code actions that already contain their source edits.
//!
//! Actions are returned only to clients that can apply versioned document changes. The engine
//! computes against the captured request document, and the server rejects a result if a later
//! editor revision overtook it before publication.

use rg_lsp_proto::{CodeActionClientCapabilities, CodeActionRequestContext};
use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::methods::DocumentMethodContext;

#[tracing::instrument(
    level = "trace", skip_all,
    fields(rg.range = ?params.range)
)]
pub(crate) async fn code_action(
    ctx: DocumentMethodContext,
    params: CodeActionParams,
    client_capabilities: CodeActionClientCapabilities,
) -> Result<Option<CodeActionResponse>> {
    // Returning no actions is safer than degrading a version-bound edit to a stale plain edit or
    // command that the client could apply to another document revision.
    if !client_capabilities.supports_eager_actions() {
        return Ok(None);
    }

    let request_context = CodeActionRequestContext::from_lsp(&params.context);
    let input = ctx.target_range(params.range)?;
    tracing::trace!("code action request received");
    let result = ctx
        .engine_client
        .query(
            "code_action",
            move |engine_client, rpc_context| async move {
                engine_client
                    .code_action(rpc_context, input, request_context)
                    .await
            },
        )
        .await;
    let mut actions = ctx.finish_target_query(result)?;
    if !client_capabilities.preferred_support {
        for action in &mut actions {
            action.is_preferred = None;
        }
    }
    tracing::trace!(result_count = actions.len(), "code action request answered");

    Ok(Some(
        actions
            .into_iter()
            .map(CodeActionOrCommand::CodeAction)
            .collect(),
    ))
}
