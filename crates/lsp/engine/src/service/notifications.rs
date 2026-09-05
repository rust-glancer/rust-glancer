use std::sync::Arc;

use rg_lsp_proto::{NotificationsServiceClient, ServiceNotification};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

pub(crate) trait ServiceNotificationPublisher: std::fmt::Debug + Send + Sync {
    fn send(&self, notification: ServiceNotification);
}

/// Fire-and-forget notifications from the service to the LSP orchestrator.
///
/// Engine and diagnostics work must not block on editor-side publication. The real RPC publisher
/// preserves notification order on one background task, so progress begin/end pairs cannot race
/// each other while callers still only enqueue best-effort work.
#[derive(Clone, Debug)]
pub struct ServiceNotificationsSink {
    publisher: Arc<dyn ServiceNotificationPublisher>,
}

#[derive(Clone, Debug)]
struct TarpcServiceNotificationPublisher {
    sender: UnboundedSender<ServiceNotification>,
}

#[derive(Clone, Debug)]
struct ChannelServiceNotificationPublisher {
    sender: UnboundedSender<ServiceNotification>,
}

impl ServiceNotificationsSink {
    pub fn new(notifications: NotificationsServiceClient) -> Self {
        Self::from_publisher(TarpcServiceNotificationPublisher::spawn(notifications))
    }

    /// Builds an in-process notification channel for embedded service consumers.
    ///
    /// The RPC process normally forwards these events over tarpc. Direct users still need the
    /// ordered lifecycle stream, especially the deferred-indexing barrier, without standing up a
    /// transport that adds no meaning inside the same process.
    pub fn channel() -> (Self, UnboundedReceiver<ServiceNotification>) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        (
            Self::from_publisher(ChannelServiceNotificationPublisher { sender }),
            receiver,
        )
    }

    pub(crate) fn from_publisher(publisher: impl ServiceNotificationPublisher + 'static) -> Self {
        Self {
            publisher: Arc::new(publisher),
        }
    }

    pub(crate) fn send(&self, notification: ServiceNotification) {
        self.publisher.send(notification);
    }
}

impl TarpcServiceNotificationPublisher {
    fn spawn(notifications: NotificationsServiceClient) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(Self::publish_in_order(notifications, receiver));

        Self { sender }
    }

    async fn publish_in_order(
        notifications: NotificationsServiceClient,
        mut receiver: UnboundedReceiver<ServiceNotification>,
    ) {
        while let Some(notification) = receiver.recv().await {
            match notifications
                .publish(tarpc::context::current(), notification)
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::debug!(
                        error = %error,
                        "LSP notifications service rejected notification"
                    );
                }
                Err(error) => {
                    tracing::debug!(error = %error, "failed to publish service notification");
                }
            }
        }
    }
}

impl ServiceNotificationPublisher for TarpcServiceNotificationPublisher {
    fn send(&self, notification: ServiceNotification) {
        if self.sender.send(notification).is_err() {
            tracing::debug!("failed to enqueue service notification");
        }
    }
}

impl ServiceNotificationPublisher for ChannelServiceNotificationPublisher {
    fn send(&self, notification: ServiceNotification) {
        if self.sender.send(notification).is_err() {
            tracing::debug!("failed to enqueue in-process service notification");
        }
    }
}
