//! Async access to the single-lane LSP analysis engine.
//!
//! RPC handlers clone `EngineHandle` and enqueue typed commands. A dedicated thread consumes those
//! commands in FIFO order, which keeps saved-project mutation, query-time materialization, and
//! package offloading from racing each other. The child modules split that thread into command
//! dispatch, project lifecycle ownership, and request-scoped query execution.

mod command;
mod dispatcher;
mod project;
mod query;

use std::{
    sync::{
        Arc,
        mpsc::{self, Sender},
    },
    thread,
    time::Instant,
};

use anyhow::Context as _;
use rg_lsp_proto::{EngineError, QueryError, QueryValue, ServiceNotification};
use rg_std::CancellationToken;
use tokio::sync::oneshot;

pub(crate) use self::{command::EngineCommand, project::ProjectConfiguration};
use self::{
    command::{EngineResponder, QueryResponder},
    dispatcher::EngineDispatcher,
};
use crate::{memory::MemoryControl, service::ServiceNotificationsSink};

/// Handle for the long-lived analysis engine.
///
/// The engine itself stays on a dedicated thread because project analysis is mostly synchronous.
/// This handle is the async side used by the RPC-facing service: each call sends one command and
/// awaits its one-shot response without exposing the project itself to async tasks.
#[derive(Clone, Debug)]
pub(crate) struct EngineHandle {
    sender: Sender<QueuedEngineCommand>,
    notifications: ServiceNotificationsSink,
}

/// Separates time spent waiting behind older commands from time spent executing this command.
#[derive(Debug)]
pub(crate) struct QueuedEngineCommand {
    pub(crate) command: EngineCommand,
    pub(crate) enqueued_at: Instant,
    pub(crate) cancellation: CancellationToken,
}

impl QueuedEngineCommand {
    fn new(command: EngineCommand) -> Self {
        Self {
            command,
            enqueued_at: Instant::now(),
            cancellation: CancellationToken::new(),
        }
    }

    fn with_cancellation(command: EngineCommand, cancellation: CancellationToken) -> Self {
        Self {
            command,
            enqueued_at: Instant::now(),
            cancellation,
        }
    }
}

/// Marks synchronous engine work obsolete when its async requester disappears.
struct RequestCancellationGuard(CancellationToken);

impl Drop for RequestCancellationGuard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl EngineHandle {
    /// Starts the in-process engine behind the service abstraction.
    pub(crate) fn spawn(
        memory_control: Arc<dyn MemoryControl>,
        notifications: ServiceNotificationsSink,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        thread::spawn({
            let sender = sender.clone();
            let notifications = notifications.clone();
            move || EngineDispatcher::new(sender, memory_control, notifications).run(receiver)
        });

        Self {
            sender,
            notifications,
        }
    }

    /// Send one typed command and wait for its response channel.
    ///
    /// Dropping the waiting RPC future closes the response endpoint and marks its cancellation
    /// token. Query lifecycle code uses the endpoint to skip queued work, while synchronous
    /// semantic loops use the token to stop bounded work that is already running.
    async fn dispatch<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> EngineCommand,
    ) -> anyhow::Result<T>
    where
        T: Send + 'static,
    {
        let (respond_to, response) = oneshot::channel();
        let cancellation = CancellationToken::new();
        let _cancellation_guard = RequestCancellationGuard(cancellation.clone());
        self.sender
            .send(QueuedEngineCommand::with_cancellation(
                build(respond_to),
                cancellation,
            ))
            .context("send LSP engine command")?;

        response.await.context("receive LSP engine response")
    }

    pub(crate) async fn request<T>(
        &self,
        build: impl FnOnce(EngineResponder<T>) -> EngineCommand,
    ) -> anyhow::Result<T>
    where
        T: Send + 'static,
    {
        self.dispatch(build).await?
    }

    /// Send a semantic query and keep execution failures inside the query error model.
    pub(crate) async fn query<T>(
        &self,
        build: impl FnOnce(QueryResponder<T>) -> EngineCommand,
    ) -> Result<QueryValue<T>, QueryError>
    where
        T: Send + 'static,
    {
        match self.dispatch(build).await {
            Ok(result) => result,
            Err(error) => Err(QueryError::Internal(EngineError::from(error))),
        }
    }

    /// Refresh semantic presentation after a saved-project change completed.
    ///
    /// Editor edit/save refreshes originate at server ingress. External filesystem changes still
    /// originate here because only the engine knows when their project replacement is complete.
    pub(crate) fn refresh_inlay_hints(&self) {
        self.notifications
            .send(ServiceNotification::InlayHintRefresh);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future as _,
        sync::mpsc,
        task::{Context, Poll},
    };

    use futures::task::noop_waker_ref;
    use rg_lsp_proto::ServiceNotification;

    use super::{EngineCommand, EngineHandle};
    use crate::service::{ServiceNotificationPublisher, ServiceNotificationsSink};

    #[derive(Debug)]
    struct NoopNotifications;

    impl ServiceNotificationPublisher for NoopNotifications {
        fn send(&self, _notification: ServiceNotification) {}
    }

    #[test]
    fn dropping_dispatch_future_cancels_its_queued_command() {
        let (sender, receiver) = mpsc::channel();
        let engine = EngineHandle {
            sender,
            notifications: ServiceNotificationsSink::from_publisher(NoopNotifications),
        };
        let mut dispatch = Box::pin(engine.dispatch(EngineCommand::Shutdown));
        let mut context = Context::from_waker(noop_waker_ref());

        assert!(matches!(
            dispatch.as_mut().poll(&mut context),
            Poll::Pending
        ));
        let queued = receiver
            .try_recv()
            .expect("polling dispatch should enqueue the engine command");
        assert!(!queued.cancellation.is_cancelled());

        drop(dispatch);

        assert!(queued.cancellation.is_cancelled());
    }

    #[tokio::test]
    async fn dropping_running_query_releases_the_lane_for_the_next_request() {
        use crate::memory::{AllocatorStats, MemoryControl};
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        };
        use std::time::Duration;

        #[derive(Debug, Default)]
        struct QueryBarrier {
            armed: Mutex<Option<(mpsc::SyncSender<()>, mpsc::Receiver<()>)>>,
            purges: AtomicUsize,
        }
        impl MemoryControl for QueryBarrier {
            fn allocator_stats(&self) -> Option<AllocatorStats> {
                let barrier = self.armed.lock().expect("query barrier lock").take();
                if let Some((started, resume)) = barrier {
                    // The lifecycle has passed its queued-cancellation check and now owns cleanup.
                    started.send(()).expect("query observer exists");
                    resume
                        .recv_timeout(Duration::from_secs(5))
                        .expect("query owner releases barrier");
                }
                None
            }
            fn try_purge_allocator(&self) -> bool {
                self.purges.fetch_add(1, Ordering::SeqCst);
                false
            }
        }
        let (fixture, _) = test_fixture::fixture_crate_with_markers(
            r#"
//- /Cargo.toml
[package]
name = "running_cancellation"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Ready;
"#,
        );
        let memory = Arc::new(QueryBarrier::default());
        let engine = EngineHandle::spawn(
            memory.clone(),
            ServiceNotificationsSink::from_publisher(NoopNotifications),
        );
        engine
            .request(|respond_to| EngineCommand::Initialize {
                root: fixture.path(""),
                configuration: rg_lsp_proto::AnalysisConfig {
                    sysroot_discovery: rg_lsp_proto::SysrootDiscovery::Disabled,
                    ..Default::default()
                }
                .into(),
                respond_to,
            })
            .await
            .expect("fixture engine initializes");
        let purges_before = memory.purges.load(Ordering::SeqCst);
        let (started, observed) = mpsc::sync_channel(1);
        let (resume, paused) = mpsc::channel();
        *memory.armed.lock().expect("query barrier lock") = Some((started, paused));
        let mut abandoned = Box::pin(engine.query(|respond_to| EngineCommand::WorkspaceSymbol {
            query: "Ready".into(),
            respond_to,
        }));
        let mut context = Context::from_waker(noop_waker_ref());
        assert!(abandoned.as_mut().poll(&mut context).is_pending());
        observed
            .recv_timeout(Duration::from_secs(5))
            .expect("request reaches running lifecycle");
        let mut next = Box::pin(engine.query(|respond_to| EngineCommand::WorkspaceSymbol {
            query: "Ready".into(),
            respond_to,
        }));
        assert!(next.as_mut().poll(&mut context).is_pending());
        drop(abandoned);
        resume
            .send(())
            .expect("running request remains at its checkpoint");
        let result = tokio::time::timeout(Duration::from_secs(5), next)
            .await
            .expect("next queued request can run")
            .expect("saved project remains queryable");
        assert_eq!(result.value().len(), 1);
        assert_eq!(result.value()[0].base_symbol_information.name, "Ready");
        assert!(
            memory.purges.load(Ordering::SeqCst) > purges_before,
            "abandoned request finishes cleanup before the next command"
        );
        engine
            .request(EngineCommand::Shutdown)
            .await
            .expect("fixture engine shuts down");
    }
}
