use std::{
    fmt,
    future::Future,
    time::{Duration, Instant},
};

use anyhow::Context as _;
use rg_lsp_proto::{EngineResult, EngineServiceClient};
use tarpc::client::RpcError as TarpcRpcError;

const INDEXING_RPC_DEADLINE: Duration = Duration::from_secs(30 * 60);

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
        let result = request(self.engine_service_client.clone(), Self::context(operation))
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

    fn context(operation: &'static str) -> tarpc::context::Context {
        let mut context = tarpc::context::current();
        if Self::operation_may_rebuild_analysis(operation) {
            context.deadline = Instant::now() + INDEXING_RPC_DEADLINE;
        }
        context
    }

    fn operation_may_rebuild_analysis(operation: &'static str) -> bool {
        matches!(operation, "initialize" | "reindex_workspace" | "did_save")
    }
}

impl fmt::Debug for EngineClient {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("EngineClient").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::EngineClient;

    #[test]
    fn indexing_operations_get_long_rpc_deadline() {
        for operation in ["initialize", "reindex_workspace", "did_save"] {
            let context = EngineClient::context(operation);

            assert!(
                context.deadline > Instant::now() + Duration::from_secs(20 * 60),
                "{operation} should allow slow analysis rebuilds",
            );
        }
    }

    #[test]
    fn interactive_operations_keep_tarpc_default_deadline() {
        let context = EngineClient::context("hover");

        assert!(
            context.deadline < Instant::now() + Duration::from_secs(20),
            "interactive engine calls should keep the default short tarpc deadline",
        );
    }
}
