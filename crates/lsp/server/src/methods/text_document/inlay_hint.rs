use rg_lsp_proto::QueryError;
use tower_lsp_server::{gen_lsp_types::*, jsonrpc::Result};

use crate::methods::{self, DocumentMethodContext};

/// Keep one inlay request pending when typing overtakes its analysis.
///
/// Editors can keep their existing hints while waiting, but may clear them if ContentModified
/// becomes an empty provider result. Take a new snapshot and retry inside the same request instead.
/// The returned hints always come from a completed analysis of the final captured revision.
#[tracing::instrument(
    level = "trace", skip_all,
    fields(rg.range = ?params.range)
)]
pub(crate) async fn inlay_hint(
    mut ctx: DocumentMethodContext,
    params: InlayHintParams,
) -> Result<Option<Vec<InlayHint>>> {
    tracing::trace!("inlay hint request received");

    // Follow edits only while this LSP request is alive. Dropping the handler on client
    // cancellation also drops its engine query; no background retry or retained hint data remains.
    loop {
        let captured = ctx.captured_document();
        if captured.document_revision_watch().is_superseded() {
            let recaptured = captured
                .recapture()
                .map_err(|error| methods::temporarily_unavailable(error.reason()))?;
            tracing::trace!(
                old_revision = captured.document().revision().get(),
                new_revision = recaptured.document().revision().get(),
                "recaptured inlay request for a newer target document revision"
            );
            ctx.replace_document(recaptured);
            continue;
        }

        // The client still expects the original numeric coverage after an edit. The engine
        // handles endpoints beyond the newer text, so only the document changes between attempts.
        let input = ctx.target_range(params.range)?;
        let result = ctx
            .engine_client
            .query(
                "inlay_hint",
                move |engine_client, request_context| async move {
                    engine_client.inlay_hint(request_context, input).await
                },
            )
            .await;
        match ctx.finish_attempt(result) {
            Ok(hints) => {
                tracing::trace!(
                    result_count = hints.len(),
                    revision = ctx.captured_document().document().revision().get(),
                    "inlay hint request answered"
                );
                return Ok(Some(hints));
            }
            Err(QueryError::EditorChanged) => {
                tracing::debug!(
                    revision = ctx.captured_document().document().revision().get(),
                    "inlay hint result was overtaken before publication; recapturing"
                );
            }
            Err(error) => return Err(methods::into_lsp_error(error)),
        }
    }
}
