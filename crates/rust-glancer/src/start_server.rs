use anyhow::Context as _;

/// Starts the editor-facing LSP server over stdio.
///
/// The binary owns process-wide setup: logging, runtime construction, and selecting the transport
/// mode. The server crate owns the LSP backend and engine orchestration once the runtime exists.
pub(crate) fn start_server() -> anyhow::Result<()> {
    crate::runtime::init_tracing();
    tracing::info!("starting rust-glancer LSP server over stdio");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("while attempting to build LSP server Tokio runtime")?;

    runtime.block_on(rg_lsp_server::serve_stdio())
}
