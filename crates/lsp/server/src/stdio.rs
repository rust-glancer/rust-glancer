//! Assembly of the editor-facing LSP transport.
//!
//! The wrapper order here is part of the document model. `LspService` builds ordinary async method
//! handlers, while the outer `EditorIngress` sees every decoded request in wire order and applies
//! editor messages to stored state before those futures can be polled concurrently.
//!
//! Both sides share the editor, inlay refresher, and recent-save tracker. The completion scheduler
//! stays at ingress. A completion handler receives the captured request capability, not a second
//! scheduler handle.

use tower_lsp_server::{LspService, Server};

use crate::{
    backend::Backend,
    completion_scheduler::CompletionScheduler,
    ingress::{EditorIngress, EditorStateHandle},
    inlay_refresher::InlayRefresher,
    recent_editor_saves::RecentEditorSaves,
};

/// Serves the LSP protocol over this process' stdin/stdout streams.
///
/// Runtime and tracing setup stay in the executable. This crate owns the LSP transport shape and
/// engine orchestration, but not process-wide application initialization.
pub async fn serve_stdio() -> anyhow::Result<()> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let inlay_refresher = InlayRefresher::default();
    let recent_editor_saves = RecentEditorSaves::default();
    let editor = EditorStateHandle::default();
    let completion_scheduler = CompletionScheduler::default();

    // First build the normal LSP handler service around the shared boundary owners.
    let (service, socket) = LspService::new({
        let recent_editor_saves = recent_editor_saves.clone();
        let editor = editor.clone();
        let inlay_refresher = inlay_refresher.clone();
        move |client| {
            inlay_refresher.bind(client.clone());
            Backend::new(client, recent_editor_saves, editor, inlay_refresher)
        }
    });

    // Keep ordered ingress outside `LspService`: its `Service::call` must run before the server
    // places the handler future into its concurrently-polled request set.
    let service = EditorIngress::new(
        service,
        editor,
        inlay_refresher,
        recent_editor_saves,
        completion_scheduler,
    );

    Server::new(stdin, stdout, socket).serve(service).await;

    Ok(())
}
