//! RPC transport and project availability for one engine process.
//!
//! Availability is separate from engine-side execution ordering. Semantic RPC methods still enter
//! the engine's single FIFO command lane, while lightweight document lifecycle methods update
//! their own async state directly. This module only controls what the server does around either
//! kind of RPC:
//!
//! - interactive requests use `EngineClient::query`, which returns an empty result while a saved
//!   project update is active and abandons a response that crosses such an update;
//! - save and reindex requests use `call_indexing` or `notify_indexing`, which own the matching
//!   `Indexing -> Ready/Failed` status transition through actual engine completion;
//! - startup, shutdown, and document lifecycle notifications use the unconditional transport
//!   helpers because project availability does not govern those protocol messages;
//! - the native watcher starts an indexing activity before its quiet period, then transfers that
//!   activity to `call_with_indexing_activity` when it submits the settled batch.

use std::{
    fmt,
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context as _;
use rg_lsp_proto::{EngineResult, EngineServiceClient};
use tarpc::client::RpcError as TarpcRpcError;
use tokio::sync::watch;

use self::availability::EngineAvailabilityState;
pub(crate) use self::availability::{
    EngineAvailability, EngineAvailabilitySnapshot, EngineIndexingActivity,
};

mod availability;

const INDEXING_RPC_DEADLINE: Duration = Duration::from_secs(30 * 60);

/// RPC client and saved-project availability state for one engine process.
///
/// Process readiness and project readiness are different. An engine may remain alive and accept
/// lifecycle commands while its saved project is being rebuilt, but semantic queries must return
/// neutral results during that interval. Keeping both decisions here gives method handlers one
/// explicit entrypoint for each kind of call.
#[derive(Clone)]
pub(crate) struct EngineClient {
    engine_service_client: EngineServiceClient,
    availability: Arc<EngineAvailabilityState>,
}

impl EngineClient {
    pub(crate) fn new(engine_service_client: EngineServiceClient) -> Self {
        Self {
            engine_service_client,
            availability: Arc::new(EngineAvailabilityState::new()),
        }
    }

    /// Returns the project availability paired with this engine process.
    ///
    /// Process readiness and project readiness are deliberately separate. A process stays alive
    /// while a watcher batch replaces its saved project, but interactive requests must treat that
    /// interval as temporarily unavailable.
    pub(crate) fn availability(&self) -> EngineAvailability {
        self.availability.current().availability
    }

    /// Subscribe to transitions used by the active-workspace status indicator.
    pub(crate) fn availability_changes(&self) -> watch::Receiver<EngineAvailabilitySnapshot> {
        self.availability.subscribe()
    }

    /// Mark one foreground project update as pending or running.
    ///
    /// Activities are counted because an editor save, native watcher batch, and explicit reindex
    /// can overlap. The project becomes queryable only after the last overlapping activity ends.
    pub(crate) fn begin_indexing(&self) -> EngineIndexingActivity {
        self.availability.begin()
    }

    /// Call the engine without consulting or changing saved-project availability.
    ///
    /// "Unconditional" describes only this server-side availability wrapper. It neither bypasses
    /// nor creates engine-side queueing: the invoked service method keeps its normal execution
    /// route. Callers use this for startup, shutdown, and lifecycle messages whose availability is
    /// either irrelevant or owned by a higher-level protocol.
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

    /// Run one interactive request only while the saved project is queryable.
    ///
    /// Beginning a foreground update also invalidates an RPC that is already waiting. Dropping the
    /// RPC future closes its response path, so queued engine work can observe cancellation without
    /// needing a second semantic execution lane.
    pub(crate) async fn query<T, F, Fut>(
        &self,
        operation: &'static str,
        request: F,
    ) -> anyhow::Result<T>
    where
        T: Default,
        F: FnOnce(EngineServiceClient, tarpc::context::Context) -> Fut,
        Fut: Future<Output = Result<EngineResult<T>, TarpcRpcError>>,
    {
        let request = self.call_unconditional(operation, request);
        match self.availability.run_query(operation, request).await {
            Some(result) => result,
            None => Ok(T::default()),
        }
    }

    /// Run a saved-project update and publish its availability outcome.
    ///
    /// Overlapping updates are counted by `EngineIndexingActivity`; this call returning does not
    /// make the project queryable while another save or watcher batch is still in flight. Once the
    /// RPC has been submitted, its activity lives in a detached task so cancelling the outer LSP
    /// request cannot announce readiness before the engine command finishes.
    pub(crate) async fn call_indexing<T, F, Fut>(
        &self,
        operation: &'static str,
        request: F,
    ) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(EngineServiceClient, tarpc::context::Context) -> Fut + Send + 'static,
        Fut: Future<Output = Result<EngineResult<T>, TarpcRpcError>> + Send + 'static,
    {
        let activity = self.begin_indexing();
        self.call_with_indexing_activity(activity, operation, request)
            .await
    }

    /// Submit work using an activity acquired before an external settle period.
    ///
    /// This is the native watcher's counterpart to `call_indexing`: it preserves the earlier
    /// `Indexing` transition, then gives the accepted RPC the same cancellation-safe ownership.
    pub(crate) async fn call_with_indexing_activity<T, F, Fut>(
        &self,
        activity: EngineIndexingActivity,
        operation: &'static str,
        request: F,
    ) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(EngineServiceClient, tarpc::context::Context) -> Fut + Send + 'static,
        Fut: Future<Output = Result<EngineResult<T>, TarpcRpcError>> + Send + 'static,
    {
        let engine_client = self.clone();
        activity
            .run_to_completion(
                async move { engine_client.call_unconditional(operation, request).await },
            )
            .await
    }

    /// Send a lifecycle notification regardless of saved-project availability.
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

    /// Send a notification that owns a saved-project availability transition.
    pub(crate) async fn notify_indexing<T, F, Fut>(&self, operation: &'static str, request: F)
    where
        T: Send + 'static,
        F: FnOnce(EngineServiceClient, tarpc::context::Context) -> Fut + Send + 'static,
        Fut: Future<Output = Result<EngineResult<T>, TarpcRpcError>> + Send + 'static,
    {
        if let Err(error) = self.call_indexing(operation, request).await {
            let error = format!("{error:#}");
            tracing::debug!(operation, error = %error, "engine indexing notification failed");
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
        matches!(
            operation,
            "initialize" | "reindex_workspace" | "did_save" | "external_project_paths_changed"
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
    fn indexing_operations_get_long_rpc_deadline() {
        for operation in [
            "initialize",
            "reindex_workspace",
            "did_save",
            "external_project_paths_changed",
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
