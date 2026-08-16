//! RPC transport and project status for one engine process.
//!
//! Semantic RPC methods enter the engine's single FIFO command lane with immutable editor input.
//! Project indexing state is presentation telemetry, not query-result policy. This module controls
//! what the server does around each kind of RPC:
//!
//! - interactive requests use `EngineClient::query` and preserve the engine's typed result;
//! - save and reindex requests use `call_project_update`, which owns the matching status
//!   transition through actual engine completion;
//! - startup and shutdown use the unconditional transport helpers because presentation status
//!   does not govern process lifecycle messages;
//! - the native watcher starts a project update before its quiet period, then transfers that
//!   update to `call_with_project_update` when it submits the settled batch.

use std::{
    fmt,
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context as _;
use rg_lsp_proto::{EngineError, EngineResult, EngineServiceClient, QueryError, QueryValue};
use tarpc::client::RpcError as TarpcRpcError;
use tokio::sync::watch;

use self::project_status::EngineProjectStatusState;
pub(crate) use self::project_status::{EngineProjectStatus, EngineProjectUpdate};

mod project_status;

const PROJECT_UPDATE_RPC_DEADLINE: Duration = Duration::from_secs(30 * 60);

/// RPC client and saved-project status state for one engine process.
///
/// An engine remains alive while saved project work is queued or running. Status consumers may
/// display that transition, while semantic requests preserve their ordinary FIFO relationship to
/// the mutation and return an explicit query result.
#[derive(Clone)]
pub(crate) struct EngineClient {
    engine_service_client: EngineServiceClient,
    project_status: Arc<EngineProjectStatusState>,
}

impl EngineClient {
    pub(crate) fn new(engine_service_client: EngineServiceClient) -> Self {
        Self {
            engine_service_client,
            project_status: Arc::new(EngineProjectStatusState::new()),
        }
    }

    /// Whether two route values address the same engine process and status owner.
    pub(crate) fn same_engine(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.project_status, &other.project_status)
    }

    /// Returns the saved-project status displayed for this engine process.
    pub(crate) fn project_status(&self) -> EngineProjectStatus {
        self.project_status.current()
    }

    /// Subscribe to transitions used by the active-workspace status indicator.
    pub(crate) fn project_status_changes(&self) -> watch::Receiver<EngineProjectStatus> {
        self.project_status.subscribe()
    }

    /// Mark one foreground project update as pending or running.
    ///
    /// Updates are counted because an editor save, native watcher batch, and explicit reindex
    /// can overlap. Presentation returns to ready only after the last update ends.
    pub(crate) fn begin_project_update(&self) -> EngineProjectUpdate {
        self.project_status.begin()
    }

    /// Call the engine without consulting or changing saved-project presentation status.
    ///
    /// "Unconditional" describes only this server-side status wrapper. It neither bypasses
    /// nor creates engine-side queueing: the invoked service method keeps its normal execution
    /// route. Callers use this for startup, shutdown, and other messages whose presentation state
    /// is either irrelevant or owned by a higher-level protocol.
    pub(crate) async fn call_unconditional<T, F, Fut>(
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

    /// Run one interactive request and preserve its typed query result.
    ///
    /// Saved project mutations and queries already share one engine command lane. Server-side
    /// status transitions must not manufacture feature defaults or invalidate a coherent engine
    /// response independently from that lane.
    pub(crate) async fn query<T, F, Fut>(
        &self,
        operation: &'static str,
        request: F,
    ) -> Result<QueryValue<T>, QueryError>
    where
        F: FnOnce(EngineServiceClient, tarpc::context::Context) -> Fut,
        Fut: Future<Output = Result<Result<QueryValue<T>, QueryError>, TarpcRpcError>>,
    {
        match request(self.engine_service_client.clone(), Self::context(operation)).await {
            Ok(result) => result,
            Err(error) => {
                let error = anyhow::Error::new(error)
                    .context(format!("while attempting to call engine RPC `{operation}`"));
                Err(QueryError::Internal(EngineError::from(error)))
            }
        }
    }

    /// Run a saved-project update and publish its presentation outcome.
    ///
    /// Overlapping updates are counted by `EngineProjectUpdate`; this call returning does not
    /// publish ready while another save or watcher batch is still in flight. Once the RPC has been
    /// submitted, its update lives in a detached task so cancelling the outer LSP request cannot
    /// announce readiness before the engine command finishes.
    pub(crate) async fn call_project_update<T, F, Fut>(
        &self,
        operation: &'static str,
        request: F,
    ) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(EngineServiceClient, tarpc::context::Context) -> Fut + Send + 'static,
        Fut: Future<Output = Result<EngineResult<T>, TarpcRpcError>> + Send + 'static,
    {
        let update = self.begin_project_update();
        self.call_with_project_update(update, operation, request)
            .await
    }

    /// Submit work using an update acquired before an external settle period.
    ///
    /// This is the native watcher's counterpart to `call_project_update`: it preserves the earlier
    /// status transition, then gives the accepted RPC the same cancellation-safe ownership.
    pub(crate) async fn call_with_project_update<T, F, Fut>(
        &self,
        update: EngineProjectUpdate,
        operation: &'static str,
        request: F,
    ) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(EngineServiceClient, tarpc::context::Context) -> Fut + Send + 'static,
        Fut: Future<Output = Result<EngineResult<T>, TarpcRpcError>> + Send + 'static,
    {
        let engine_client = self.clone();
        update
            .run_to_completion(
                async move { engine_client.call_unconditional(operation, request).await },
            )
            .await
    }

    /// Send a best-effort notification regardless of saved-project presentation status.
    ///
    /// LSP notifications have no response channel on which to surface an engine failure, so this
    /// waits for the RPC and records a debug message instead.
    pub(crate) async fn notify<T, F, Fut>(&self, operation: &'static str, request: F)
    where
        F: FnOnce(EngineServiceClient, tarpc::context::Context) -> Fut,
        Fut: Future<Output = Result<EngineResult<T>, TarpcRpcError>>,
    {
        if let Err(error) = self.call_unconditional(operation, request).await {
            let error = format!("{error:#}");
            tracing::debug!(operation, error = %error, "engine notification failed");
        }
    }

    fn context(operation: &'static str) -> tarpc::context::Context {
        let mut context = tarpc::context::current();
        if Self::operation_may_rebuild_analysis(operation) {
            context.deadline = Instant::now() + PROJECT_UPDATE_RPC_DEADLINE;
        }
        context
    }

    fn operation_may_rebuild_analysis(operation: &'static str) -> bool {
        matches!(
            operation,
            "initialize" | "reindex_workspace" | "did_save" | "external_project_changes"
        )
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
    fn project_update_operations_get_long_rpc_deadline() {
        for operation in [
            "initialize",
            "reindex_workspace",
            "did_save",
            "external_project_changes",
        ] {
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
