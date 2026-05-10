use std::{net::SocketAddr, sync::Arc};

use anyhow::Context as _;
use futures::prelude::*;
use rg_lsp_proto::{EngineService, NotificationsServiceClient};
use tarpc::{
    client, serde_transport::tcp, server::BaseChannel, server::Channel as _,
    tokio_serde::formats::Json,
};

use crate::{Service, memory::MemoryControl, service::ServiceNotificationsSink};

/// Runs one engine process and serves the engine RPC API over a caller-provided connection.
///
/// The LSP server owns process lifetime and binds the loopback sockets. The child connects back to
/// those sockets so stdout remains reserved for the parent LSP server's JSON-RPC stream.
pub async fn run_rpc(
    memory_control: Arc<dyn MemoryControl>,
    engine_addr: SocketAddr,
    notifications_addr: SocketAddr,
) -> anyhow::Result<()> {
    // Initialize the client for notifications service.
    let notifications = {
        // The LSP server hosts this side so the engine can report progress, diagnostics, and logs
        // without knowing anything about editor-facing protocols.
        let mut transport = tcp::connect(notifications_addr, Json::default);
        transport.config_mut().max_frame_length(usize::MAX);
        let transport = transport
            .await
            .context("while attempting to connect to LSP notifications RPC")?;
        NotificationsServiceClient::new(client::Config::default(), transport).spawn()
    };

    // Initialize the service.
    let service = {
        // Notifications are routed through the callback client. This keeps the engine worker and
        // diagnostics subsystem independent from how the LSP server eventually publishes them.
        let notifications = ServiceNotificationsSink::new(notifications);
        Service::spawn(memory_control, notifications)
    };

    // Initialize transport for engine service.
    let engine_transport = {
        // The LSP server binds the socket and then acts as the client. The worker only connects
        // back and serves requests over the initialized transport.
        let mut transport = tcp::connect(engine_addr, Json::default);
        transport.config_mut().max_frame_length(usize::MAX);
        transport
            .await
            .context("while attempting to connect to LSP engine RPC")?
    };

    // Serve the engine RPC API.
    {
        // Each request gets its own task, matching tarpc's usual server pattern and avoiding one
        // slow request from blocking unrelated engine calls.
        BaseChannel::with_defaults(engine_transport)
            .execute(service.serve())
            .for_each(|response| async move {
                tokio::spawn(response);
            })
            .await;
    }

    Ok(())
}
