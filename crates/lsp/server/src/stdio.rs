use tower_lsp_server::{LspService, Server};

use crate::{backend::Backend, engine_process::EngineProcess};

/// Serves the LSP protocol over this process' stdin/stdout streams.
///
/// Runtime and tracing setup stay in the executable. This crate owns the LSP transport shape and
/// engine orchestration, but not process-wide application initialization.
pub async fn serve_stdio() -> anyhow::Result<()> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(move |client| {
        let engine = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(EngineProcess::spawn(client.clone()))
        })
        .expect("rust-glancer engine process should start before accepting LSP requests");
        Backend::new(client, engine)
    });

    Server::new(stdin, stdout, socket).serve(service).await;

    Ok(())
}
