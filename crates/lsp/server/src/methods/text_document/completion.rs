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
//!
//! Resolving an import is a separate request for a row the editor already has. An updated
//! target buffer may need a new cursor position. Changes in any open buffer can require
//! a fresh check that the row still applies.

use rg_lsp_proto::{
    CompletionCandidate, CompletionImportCandidate, CompletionImportData, QueryError,
};
use tower_lsp_server::{gen_lsp_types::*, jsonrpc::Result};

use crate::{
    completion_scheduler::CompletionAttemptOutcome,
    methods::{self, CompletionMethodContext, DocumentMethodContext},
};

#[tracing::instrument(
    level = "trace", skip_all,
    fields(
        rg.position = ?params.text_document_position_params.position,
    )
)]
pub(crate) async fn completion(
    mut ctx: CompletionMethodContext,
    params: CompletionParams,
) -> Result<Option<CompletionResponse>> {
    let request = ctx.request.clone();
    let mut position = params.text_document_position_params.position;
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
        if captured.editor_revision_watch().is_superseded() {
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
            ctx.document.replace_document(recaptured);
            position = rebased_position;
            continue;
        }

        let input = ctx.input(position)?;
        let invalidation = captured.editor_revision_watch();
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
                match ctx.document.finish_attempt(result) {
                    Ok(completions) => {
                        tracing::trace!(
                            result_count = completions.items.len(),
                            "completion request answered"
                        );
                        let uri = &params.text_document_position_params.text_document.uri;
                        let items = completions
                            .items
                            .into_iter()
                            .map(|CompletionCandidate { mut item, import }| {
                                if let Some(import) = import {
                                    item.data = Some(
                                        serde_json::to_value(CompletionImportData {
                                            uri: uri.clone(),
                                            position,
                                            document: captured.document().target().clone(),
                                            editor_revision: captured.open_documents_revision(),
                                            site: completions.import_site.clone(),
                                            import,
                                        })
                                        .map_err(|error| methods::internal_error(error.into()))?,
                                    );
                                }
                                Ok(item)
                            })
                            .collect::<Result<_>>()?;
                        return Ok(Some(CompletionResponse::CompletionList(CompletionList {
                            items,
                            is_incomplete: completions.is_incomplete,
                            ..Default::default()
                        })));
                    }
                    Err(QueryError::EditorChanged) => {
                        tracing::debug!(
                            path = %captured.document().path().display(),
                            session = captured.document().session().get(),
                            revision = captured.document().revision().get(),
                            "completion result was overtaken before publication; recapturing"
                        );
                    }
                    Err(error) => return Err(methods::into_lsp_error(error)),
                }
            }
        }
    }
}

/// Compute the import edit for one selected candidate. Resolve has no scheduler lease because
/// previewing a row must not replace a newer request for the completion list.
pub(crate) async fn resolve_import(
    ctx: DocumentMethodContext,
    mut item: CompletionItem,
    data: CompletionImportData,
) -> Result<CompletionItem> {
    item.additional_text_edits = None;
    let captured = ctx.captured_document();
    let current = captured.document().target();
    if current.session() != data.document.session() || current.path() != data.document.path() {
        return Ok(item);
    }

    // A sibling edit can change which methods apply without moving the cursor. Only edits
    // to the target require finding the position again.
    let (position, inline_module_path) = if current == &data.document {
        (data.position, None)
    } else {
        let text = captured.document().text();
        let Some(site) = data.site else {
            return Ok(item);
        };
        let Some(offset) = site.offset_in(text, &item.label) else {
            return Ok(item);
        };
        let prefix = &text[..offset];
        let column = prefix.rsplit_once('\n').map_or(prefix, |(_, line)| line);
        let (Ok(line), Ok(column)) = (
            u32::try_from(prefix.bytes().filter(|&byte| byte == b'\n').count()),
            u32::try_from(column.encode_utf16().count()),
        ) else {
            return Ok(item);
        };
        (Position::new(line, column), Some(site.inline_module_path))
    };
    let candidate = if inline_module_path.is_some()
        || captured.open_documents_revision() != data.editor_revision
    {
        let Some(kind) = item.kind else {
            return Ok(item);
        };
        Some(CompletionImportCandidate {
            label: item.label.clone(),
            kind,
            inline_module_path,
        })
    } else {
        None
    };
    let Ok(input) = ctx.position(position) else {
        return Ok(item);
    };
    let result = ctx
        .engine_client
        .query("resolve_completion_import", |client, context| async move {
            client
                .resolve_completion_import(context, input, data.import, candidate)
                .await
        })
        .await;
    // The edit belongs to the capture we sent. If the editor overtook it, skip the edit.
    match ctx.finish_attempt(result) {
        Ok(edit) => item.additional_text_edits = edit.map(|edit| vec![edit]),
        Err(
            QueryError::EditorChanged
            | QueryError::SavedSourceChanged
            | QueryError::TemporarilyUnavailable,
        ) => {}
        Err(error) => return Err(methods::into_lsp_error(error)),
    }
    Ok(item)
}
