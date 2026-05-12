use std::{net::SocketAddr, sync::Arc};

use anyhow::Context as _;

/// Starts one engine subprocess and connects it to the parent LSP server.
///
/// This is intentionally hidden CLI plumbing. The parent server decides when to spawn engines; this
/// mode only initializes process-level concerns and then hands control to the engine RPC server.
pub(crate) fn start_engine(
    engine_addr: SocketAddr,
    notifications_addr: SocketAddr,
) -> anyhow::Result<()> {
    crate::logging::init_lsp_tracing(crate::logging::LogComponent::engine_from_env());

    let memory_control: Arc<dyn rg_lsp_engine::MemoryControl> =
        Arc::new(crate::memory::memory_control());
    tracing::info!(
        allocator = memory_control.allocator_name(),
        allocator_purge_enabled = memory_control.allocator_purge_enabled(),
        "starting rust-glancer LSP engine process"
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("while attempting to build LSP engine Tokio runtime")?;

    runtime.block_on(rg_lsp_engine::run_rpc(
        memory_control,
        engine_addr,
        notifications_addr,
    ))
}
