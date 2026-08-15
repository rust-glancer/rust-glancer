//! Retries one completion request when the editor changes while its analysis is running.
//!
//! Two lifetimes matter here:
//!
//! - A *logical request* is one completion message from the editor.
//! - A *semantic attempt* is one engine query made from one captured document revision.
//!
//! For example, a request may start at `RwLo|`, then `didChange` inserts `ck` while the engine is
//! still working. The same logical request moves its cursor to `RwLock|` and starts a second
//! semantic attempt with the newer document. A separate completion message from the editor
//! replaces the whole logical request, including any attempt it is running.

use tower_lsp_server::{jsonrpc::Result, ls_types::*};

use crate::{
    completion_scheduler::CompletionAttemptOutcome,
    methods::{self, CompletionMethodContext, DocumentQueryStatus},
};

#[tracing::instrument(
    level = "trace", skip_all,
    fields(
        rg.position = ?params.text_document_position.position,
    )
)]
pub(crate) async fn completion(
    mut ctx: CompletionMethodContext,
    params: CompletionParams,
) -> Result<Option<CompletionResponse>> {
    let request = ctx.request.clone();
    let mut position = params.text_document_position.position;
    tracing::trace!("completion request received");

    // One loop owns every retry for this client request. When the target document advances,
    // replace the old capture, move the cursor through the recorded edits, and submit another
    // engine query. Drop the previous capture after each successful move so old edit links live no
    // longer than this request needs them.
    loop {
        if request.is_replaced() {
            return Err(methods::temporarily_unavailable(
                "completion request was replaced by a newer request point",
            ));
        }

        let captured = ctx.document.captured_document().clone();
        if captured.document_revision_watch().is_superseded() {
            let (recaptured, rebased_position) =
                captured.recapture_position(position).map_err(|error| {
                    tracing::debug!(
                        path = %captured.document().path().display(),
                        session = captured.document().session().get(),
                        reason = error.reason(),
                        "completion position could not be recaptured"
                    );
                    methods::temporarily_unavailable(error.reason())
                })?;
            tracing::trace!(
                old_revision = captured.document().revision().get(),
                new_revision = recaptured.document().revision().get(),
                old_position = ?position,
                new_position = ?rebased_position,
                "rebased completion for a newer target document revision"
            );
            ctx.replace_document(recaptured);
            position = rebased_position;
            continue;
        }

        let input = ctx.input(position)?;
        let invalidation = captured.document_revision_watch();
        let client_capabilities = ctx.client_capabilities;
        let engine_client = ctx.document.engine_client.clone();
        let outcome = request
            .submit_attempt(input, invalidation, move |input| async move {
                engine_client
                    .query(
                        "completion",
                        move |engine_client, request_context| async move {
                            engine_client
                                .completion(request_context, input, client_capabilities)
                                .await
                        },
                    )
                    .await
            })
            .await;

        match outcome {
            CompletionAttemptOutcome::DocumentAdvanced => {
                tracing::debug!(
                    path = %captured.document().path().display(),
                    session = captured.document().session().get(),
                    revision = captured.document().revision().get(),
                    "completion request will recapture after target document advancement"
                );
            }
            CompletionAttemptOutcome::Replaced => {
                return Err(methods::temporarily_unavailable(
                    "completion request was replaced by a newer request point",
                ));
            }
            CompletionAttemptOutcome::Completed(result) => {
                if request.is_replaced() {
                    return Err(methods::temporarily_unavailable(
                        "completion request was replaced by a newer request point",
                    ));
                }
                match ctx.finish_attempt(result)? {
                    DocumentQueryStatus::Current(completions) => {
                        if completions.coverage().is_partial() {
                            tracing::debug!(
                                path = %captured.document().path().display(),
                                "completion used current syntax and saved global semantics; saving may improve global completeness"
                            );
                        }
                        tracing::trace!(
                            result_count = completions.value().len(),
                            coverage = ?completions.coverage(),
                            "completion request answered"
                        );
                        return Ok(Some(incomplete_response(completions.into_value())));
                    }
                    DocumentQueryStatus::EditorChanged => {
                        tracing::debug!(
                            path = %captured.document().path().display(),
                            session = captured.document().session().get(),
                            revision = captured.document().revision().get(),
                            "completion result was overtaken before publication; recapturing"
                        );
                    }
                }
            }
        }
    }
}

/// Completion results stay incomplete so the client may ask again as the cursor advances.
fn incomplete_response(items: Vec<CompletionItem>) -> CompletionResponse {
    CompletionResponse::List(CompletionList {
        is_incomplete: true,
        items,
    })
}

#[cfg(test)]
mod tests;
