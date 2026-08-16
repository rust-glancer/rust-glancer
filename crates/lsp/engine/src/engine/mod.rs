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
}

impl QueuedEngineCommand {
    fn new(command: EngineCommand) -> Self {
        Self {
            command,
            enqueued_at: Instant::now(),
        }
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
    /// Dropping the waiting RPC future closes the response endpoint. Query lifecycle code notices
    /// that before starting queued work and can expose the same liveness at feature checkpoints,
    /// so cancellation does not need a second protocol or another request-state owner.
    async fn dispatch<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> EngineCommand,
    ) -> anyhow::Result<T>
    where
        T: Send + 'static,
    {
        let (respond_to, response) = oneshot::channel();
        self.sender
            .send(QueuedEngineCommand::new(build(respond_to)))
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
