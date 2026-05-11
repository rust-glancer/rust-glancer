use std::{fmt, net::SocketAddr, process::Stdio, sync::Arc, time::Duration};

use anyhow::Context as _;
use futures::prelude::*;
use rg_lsp_proto::{EngineServiceClient, NotificationsService};
use tarpc::{
    client::Config as TarpcClientConfig,
    serde_transport::tcp,
    server::{BaseChannel, Channel as _},
    tokio_serde::formats::Json,
};
use tokio::{process::Child, sync::Mutex};
use tower_lsp_server::Client as LspClient;

use crate::{engine_client::EngineClient, notifications::NotificationsPublisher};

const ENGINE_CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);

/// Process-backed handle to one engine owned by the LSP server.
///
/// Process lifetime lives here; request-specific logic belongs to method handlers through
/// `EngineClient`, while multi-engine routing lives one level up in the registry.
#[derive(Clone)]
pub(crate) struct EngineProcess {
    engine_client: EngineClient,
    // Kept alive so `kill_on_drop` remains tied to the server-side engine handle.
    _child: Arc<Mutex<Child>>,
}

impl EngineProcess {
    pub(crate) async fn spawn(lsp_client: LspClient) -> anyhow::Result<Self> {
        // Initialize transport for engine service.
        let (mut engine_listener, engine_addr) = {
            // Both JSON and TCP are chosen for convenience of debugging, not that we need too much speed.
            // Even though it's the engine that "hosts" the service, it doesn't matter which
            // side starts the socket; it's bidirectional and `tarpc` can work over any initialized
            // transport.
            let mut listener = tcp::listen("127.0.0.1:0", Json::default)
                .await
                .expect("Failed to bind a TCP listener");
            listener.config_mut().max_frame_length(usize::MAX);
            let addr = listener.local_addr();
            (listener, addr)
        };

        // Initialize transport for notifications service.
        // The engine uses a second connection to send progress, diagnostics, and logs back to
        // the LSP server without mixing callback traffic into request/response flow.
        let (mut notifications_listener, notifications_addr) = {
            let mut listener = tcp::listen("127.0.0.1:0", Json::default)
                .await
                .expect("Failed to bind a TCP listener");
            listener.config_mut().max_frame_length(usize::MAX);
            let addr = listener.local_addr();
            (listener, addr)
        };

        // Spawn the engine subprocess.
        let child = Self::spawn_worker(engine_addr, notifications_addr)?;

        // Spawn the notifications publisher.
        {
            // Accept the notification connection in the background. The main initialization path
            // only needs the engine client below; callback delivery can become ready independently.
            let publisher = NotificationsPublisher::new(lsp_client);
            tokio::spawn(async move {
                let accept =
                    tokio::time::timeout(ENGINE_CONNECTION_TIMEOUT, notifications_listener.next())
                        .await;
                let transport = match accept {
                    Ok(Some(Ok(transport))) => transport,
                    Ok(Some(Err(error))) => {
                        tracing::error!(error = %error, "failed to accept notifications RPC connection");
                        return;
                    }
                    Ok(None) => {
                        tracing::error!(
                            "notifications RPC listener closed before engine connected"
                        );
                        return;
                    }
                    Err(_) => {
                        tracing::error!(
                            "timed out waiting for engine notifications RPC connection"
                        );
                        return;
                    }
                };

                BaseChannel::with_defaults(transport)
                    .execute(publisher.serve())
                    .for_each(|response| async move {
                        tokio::spawn(response);
                    })
                    .await;
            });
        }

        // Initialize the engine RPC client.
        let engine_client = {
            // Wait for the worker to connect its request channel before constructing the backend.
            // Once this returns, method handlers can send engine RPCs normally.
            let engine_transport =
                tokio::time::timeout(ENGINE_CONNECTION_TIMEOUT, engine_listener.next())
                    .await
                    .context("while attempting to wait for engine RPC connection")?
                    .ok_or_else(|| {
                        anyhow::anyhow!("engine RPC listener closed before engine connected")
                    })?
                    .context("while attempting to accept engine RPC connection")?;
            let engine_service_client =
                EngineServiceClient::new(TarpcClientConfig::default(), engine_transport).spawn();
            EngineClient::new(engine_service_client)
        };

        Ok(Self {
            engine_client,
            _child: Arc::new(Mutex::new(child)),
        })
    }

    pub(crate) fn engine_client(&self) -> &EngineClient {
        &self.engine_client
    }

    fn spawn_worker(
        engine_addr: SocketAddr,
        notifications_addr: SocketAddr,
    ) -> anyhow::Result<Child> {
        let executable = std::env::current_exe()
            .context("while attempting to locate rust-glancer executable")?;
        let args = [
            "lsp-engine".to_string(),
            "--engine-addr".to_string(),
            engine_addr.to_string(),
            "--notifications-addr".to_string(),
            notifications_addr.to_string(),
        ];

        tokio::process::Command::new(executable)
            .args(args)
            // The parent LSP server owns stdout for JSON-RPC. The engine may log to stderr, but it
            // must never inherit stdout and accidentally corrupt the LSP stream.
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .context("while attempting to spawn rust-glancer engine process")
    }
}

impl fmt::Debug for EngineProcess {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("EngineProcess").finish_non_exhaustive()
    }
}
