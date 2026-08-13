//! Prepares editor-related LSP messages before their async handlers start.
//!
//! `EditorIngress::call` sees decoded messages one at a time and in the order they arrived. It
//! immediately applies editor changes, opens or closes sessions, and takes document snapshots for
//! requests. It also records which completion request is the newest for each document session.
//! Only after that work is done does the inner LSP service create the async handler future.
//!
//! Data needed by the handler is attached to that future through task-local storage. Slower work,
//! such as finding an engine or sending a save to it, remains in the normal method handler.
//! Open/save/close handlers wait only for earlier work on the same document, so work on unrelated
//! documents may continue independently.

use std::{
    sync::Mutex,
    task::{Context, Poll},
};

use futures::future::BoxFuture;
use tower::Service;
use tower_lsp_server::{
    jsonrpc::{FromParams, Request},
    ls_types::{
        CompletionParams, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
        DidOpenTextDocumentParams, DidSaveTextDocumentParams, Uri,
    },
};

use crate::{
    completion_scheduler::{CompletionRequest, CompletionScheduler},
    inlay_refresher::InlayRefresher,
    methods,
    recent_editor_saves::RecentEditorSaves,
};

use super::state::{
    CapturedDocument, DocumentUnavailable, EditorStateHandle, SequencedLifecycleEvent,
};

tokio::task_local! {
    /// Data prepared for one LSP message and available only while that message's handler runs.
    ///
    /// `EditorIngress::prepare_call` creates this value before the handler future is allowed to
    /// start. `EditorIngress::call` then places it in Tokio's task-local storage while that handler
    /// runs. It remains available across `.await` points, but it is not shared with handlers for
    /// other messages. The value disappears when its handler finishes or is cancelled.
    ///
    /// Handler code reads this data through three helpers:
    ///
    /// - `lifecycle_event` waits for an earlier open/save/close handler for the same document, then
    ///   returns the async work assigned to this handler.
    /// - `document_request` takes the immutable document snapshot chosen for this request. It is
    ///   one-shot so a method cannot later switch to another editor revision by accident.
    /// - `completion_request` returns the request identity used to detect a newer completion.
    ///
    /// When an open/save/close handler finishes, `EditorIngress::call` marks its work as finished
    /// and allows the next handler for that document to continue. Dropping a cancelled handler
    /// releases the next one as well. This task-local only passes data to the matching handler;
    /// `EditorStateHandle` remains the place that stores text, sessions, and revisions.
    static CURRENT_INGRESS_CALL: IngressCall;
}

/// Wraps the LSP service so order-sensitive editor work happens before async handlers start.
///
/// This must remain the outer service passed to `tower_lsp_server::Server`. Wrapping at a deeper
/// async layer would no longer let it process decoded messages one at a time and in arrival order.
#[derive(Debug)]
pub(crate) struct EditorIngress<S> {
    inner: S,
    editor: EditorStateHandle,
    inlay_refresher: InlayRefresher,
    recent_editor_saves: RecentEditorSaves,
    completion_scheduler: CompletionScheduler,
}

impl<S> EditorIngress<S> {
    pub(crate) fn new(
        inner: S,
        editor: EditorStateHandle,
        inlay_refresher: InlayRefresher,
        recent_editor_saves: RecentEditorSaves,
        completion_scheduler: CompletionScheduler,
    ) -> Self {
        Self {
            inner,
            editor,
            inlay_refresher,
            recent_editor_saves,
            completion_scheduler,
        }
    }

    /// Prepare the editor state or request snapshot needed before this message's handler starts.
    ///
    /// Open/change/save/close perform their order-sensitive state work here. Document requests
    /// take an immutable snapshot here. Engine and registry calls remain in the async handlers.
    fn prepare_call(&self, request: &Request) -> IngressCall {
        match request.method() {
            "textDocument/didOpen" => {
                let Some(params) = request_params::<DidOpenTextDocumentParams>(request) else {
                    return IngressCall::Other;
                };
                let Some(path) = methods::uri_to_path(&params.text_document.uri) else {
                    return IngressCall::Other;
                };
                IngressCall::Lifecycle(self.editor.open(
                    path,
                    Some(params.text_document.version),
                    params.text_document.text,
                ))
            }
            "textDocument/didChange" => {
                let Some(params) = request_params::<DidChangeTextDocumentParams>(request) else {
                    return IngressCall::Other;
                };
                let Some(path) = methods::uri_to_path(&params.text_document.uri) else {
                    return IngressCall::Other;
                };
                let client_version = Some(params.text_document.version);
                let content_change_count = params.content_changes.len();
                match self
                    .editor
                    .change(&path, client_version, &params.content_changes)
                {
                    Ok(true) => {}
                    Ok(false) => {
                        tracing::debug!(
                            path = %path.display(),
                            "ignored document change without an open editor session"
                        );
                        return IngressCall::Other;
                    }
                    Err(error) => {
                        tracing::warn!(
                            path = %path.display(),
                            ?client_version,
                            content_change_count,
                            reason = %error,
                            "document change could not produce exact synchronized text"
                        );
                    }
                }

                self.inlay_refresher.document_changed();
                IngressCall::Other
            }
            "textDocument/didSave" => {
                let Some(params) = request_params::<DidSaveTextDocumentParams>(request) else {
                    return IngressCall::Other;
                };
                let Some(path) = methods::uri_to_path(&params.text_document.uri) else {
                    return IngressCall::Other;
                };
                // Record the save before separately scheduled file-watcher work can observe the
                // same disk write. If this waited for the async didSave handler, the watcher could
                // run first and mistake the editor's save for an external change.
                self.recent_editor_saves.record_editor_save(&path);
                let Some(sequenced) = self.editor.save(&path, params.text) else {
                    tracing::debug!(
                        path = %path.display(),
                        "ignored document save without an open editor session"
                    );
                    return IngressCall::Other;
                };

                IngressCall::Lifecycle(sequenced)
            }
            "textDocument/didClose" => {
                let Some(params) = request_params::<DidCloseTextDocumentParams>(request) else {
                    return IngressCall::Other;
                };
                let Some(path) = methods::uri_to_path(&params.text_document.uri) else {
                    return IngressCall::Other;
                };
                self.editor
                    .close(&path)
                    .map(IngressCall::Lifecycle)
                    .unwrap_or_else(|| {
                        tracing::debug!(
                            path = %path.display(),
                            "ignored document close without an open editor session"
                        );
                        IngressCall::Other
                    })
            }
            "textDocument/completion" => {
                let Some(params) = request_params::<CompletionParams>(request) else {
                    return IngressCall::Other;
                };
                let Some(path) =
                    methods::uri_to_path(&params.text_document_position.text_document.uri)
                else {
                    return IngressCall::Other;
                };
                let document = self.editor.document(Some(path));
                let completion = document.as_ref().ok().map(|captured| {
                    self.completion_scheduler
                        .capture_request(captured, params.text_document_position.position)
                });
                IngressCall::Document {
                    document: Mutex::new(Some(document)),
                    completion,
                }
            }
            method if is_document_request(method) => {
                let path = request_document_uri(request).and_then(|uri| methods::uri_to_path(&uri));
                IngressCall::Document {
                    document: Mutex::new(Some(self.editor.document(path))),
                    completion: None,
                }
            }
            _ => IngressCall::Other,
        }
    }
}

impl<S> Service<Request> for EditorIngress<S>
where
    S: Service<Request> + Send + 'static,
    S::Future: Send + 'static,
    S::Response: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        // The server invokes this method in decoded message order. Finish the order-sensitive
        // editor work before this handler may run at the same time as handlers for later messages.
        let ingress_call = self.prepare_call(&request);
        let lifecycle = ingress_call.lifecycle();
        let future = self.inner.call(request);

        Box::pin(async move {
            let result = CURRENT_INGRESS_CALL.scope(ingress_call, future).await;
            if let Some(lifecycle) = lifecycle {
                lifecycle.finish();
            }
            result
        })
    }
}

/// Data or remaining async work prepared for one LSP handler.
///
/// For open/save/close, the immediate editor work has already happened: open recorded its session,
/// save captured its text, or close marked its session closed. This value carries the async work
/// that remains. For document requests, it carries one immutable snapshot and, for completion, the
/// identity used to detect a newer request.
#[derive(Debug)]
enum IngressCall {
    Other,
    Lifecycle(SequencedLifecycleEvent),
    Document {
        document: Mutex<Option<Result<CapturedDocument, DocumentUnavailable>>>,
        completion: Option<CompletionRequest>,
    },
}

impl IngressCall {
    fn lifecycle(&self) -> Option<SequencedLifecycleEvent> {
        match self {
            Self::Lifecycle(sequenced) => Some(sequenced.clone()),
            Self::Other | Self::Document { .. } => None,
        }
    }
}

/// Wait for earlier open/save/close work on this document, then return this handler's work.
pub(crate) async fn lifecycle_event() -> Option<super::state::LifecycleEvent> {
    let sequenced = CURRENT_INGRESS_CALL
        .try_with(|call| match call {
            IngressCall::Lifecycle(sequenced) => Some(sequenced.clone()),
            IngressCall::Other | IngressCall::Document { .. } => None,
        })
        .ok()
        .flatten()?;
    sequenced.wait_for_previous().await;
    Some(sequenced.event())
}

/// Take the document snapshot chosen before this handler could run alongside a later edit.
///
/// `None` means a caller bypassed `EditorIngress`, which is an internal wiring error rather than
/// an ordinary unavailable document. An explicit unavailable capture is returned as `Some(Err)`.
pub(crate) fn document_request() -> Option<Result<CapturedDocument, DocumentUnavailable>> {
    CURRENT_INGRESS_CALL
        .try_with(|call| match call {
            IngressCall::Document { document, .. } => document
                .lock()
                .expect("ingress document mutex should not be poisoned")
                .take(),
            IngressCall::Other | IngressCall::Lifecycle(_) => None,
        })
        .ok()
        .flatten()
}

/// Return the identity assigned to this completion before its async handler started.
///
/// The scheduler uses this identity to tell whether a newer completion request has replaced this
/// one. If the editor changes while this request is still alive, another analysis attempt keeps
/// the same identity because it is still answering the same client request.
pub(crate) fn completion_request() -> Option<CompletionRequest> {
    CURRENT_INGRESS_CALL
        .try_with(|call| match call {
            IngressCall::Document { completion, .. } => completion.clone(),
            IngressCall::Other | IngressCall::Lifecycle(_) => None,
        })
        .ok()
        .flatten()
}

fn request_params<P>(request: &Request) -> Option<P>
where
    (P,): FromParams,
{
    <(P,) as FromParams>::from_params(request.params().cloned())
        .ok()
        .map(|(params,)| params)
}

fn request_document_uri(request: &Request) -> Option<Uri> {
    request
        .params()?
        .get("textDocument")?
        .get("uri")?
        .as_str()?
        .parse()
        .ok()
}

fn is_document_request(method: &str) -> bool {
    matches!(
        method,
        "textDocument/definition"
            | "textDocument/typeDefinition"
            | "textDocument/implementation"
            | "textDocument/references"
            | "textDocument/prepareRename"
            | "textDocument/rename"
            | "textDocument/documentHighlight"
            | "textDocument/hover"
            | "textDocument/completion"
            | "textDocument/formatting"
            | "textDocument/documentSymbol"
            | "textDocument/inlayHint"
    )
}

#[cfg(test)]
mod tests;
