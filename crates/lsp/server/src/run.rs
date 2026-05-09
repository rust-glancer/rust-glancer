use std::sync::Arc;

use tower_lsp_server::{LspService, Server};
use tracing_subscriber::EnvFilter;

use crate::{MemoryControl, backend::Backend};

/// Starts the rust-glancer LSP server over stdio.
pub fn run_stdio() -> anyhow::Result<()> {
    run_stdio_with_memory_control(())
}

/// Starts the rust-glancer LSP server with process-level memory controls.
pub fn run_stdio_with_memory_control(
    memory_control: impl MemoryControl + 'static,
) -> anyhow::Result<()> {
    let memory_control: Arc<dyn MemoryControl> = Arc::new(memory_control);
    let filter =
        EnvFilter::try_from_env("RUST_GLANCER_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();

    tracing::info!(
        allocator = memory_control.allocator_name(),
        allocator_purge_enabled = memory_control.allocator_purge_enabled(),
        "starting rust-glancer LSP server over stdio"
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        let (service, socket) =
            LspService::new(move |client| Backend::new(client, Arc::clone(&memory_control)));

        Server::new(stdin, stdout, socket).serve(service).await;

        Ok(())
    })
}
