use std::{fmt, future::Future};

use anyhow::Context as _;
use rg_lsp_proto::{EngineResult, EngineServiceClient};
use tarpc::client::RpcError as TarpcRpcError;

/// RPC client for one engine, without any knowledge of how that engine is hosted.
///
/// Method handlers own request-specific flow. This wrapper only keeps tarpc plumbing in one place:
/// context creation and unwrapping the transport/protocol result pair.
#[derive(Clone)]
pub(crate) struct EngineClient {
    engine_service_client: EngineServiceClient,
}

impl EngineClient {
    pub(crate) fn new(engine_service_client: EngineServiceClient) -> Self {
        Self {
            engine_service_client,
        }
    }

    pub(crate) async fn call<T, F, Fut>(
        &self,
        operation: &'static str,
        request: F,
    ) -> anyhow::Result<T>
    where
        F: FnOnce(EngineServiceClient, tarpc::context::Context) -> Fut,
        Fut: Future<Output = Result<EngineResult<T>, TarpcRpcError>>,
    {
        let result = request(
            self.engine_service_client.clone(),
            tarpc::context::current(),
        )
        .await
        .with_context(|| format!("while attempting to call engine RPC `{operation}`"))?;
        result.map_err(anyhow::Error::from)
    }

    pub(crate) async fn notify<T, F, Fut>(&self, operation: &'static str, request: F)
    where
        F: FnOnce(EngineServiceClient, tarpc::context::Context) -> Fut,
        Fut: Future<Output = Result<EngineResult<T>, TarpcRpcError>>,
    {
        if let Err(error) = self.call(operation, request).await {
            tracing::debug!(operation, error = %error, "engine notification failed");
        }
    }
}

impl fmt::Debug for EngineClient {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("EngineClient").finish_non_exhaustive()
    }
}
