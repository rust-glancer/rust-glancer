use std::sync::Arc;

use rg_lsp_proto::{NotificationsServiceClient, ServiceNotification};

pub(crate) trait ServiceNotificationPublisher: std::fmt::Debug + Send + Sync {
    fn send(&self, notification: ServiceNotification);
}

/// Fire-and-forget notifications from the service to the LSP orchestrator.
///
/// Engine and diagnostics work must not block on editor-side publication. Each notification is
/// published on a detached task, keeping progress and diagnostics best-effort across the process
/// boundary.
#[derive(Clone, Debug)]
pub struct ServiceNotificationsSink {
    publisher: Arc<dyn ServiceNotificationPublisher>,
}

#[derive(Clone, Debug)]
struct TarpcServiceNotificationPublisher {
    notifications: NotificationsServiceClient,
}

impl ServiceNotificationsSink {
    pub fn new(notifications: NotificationsServiceClient) -> Self {
        Self::from_publisher(TarpcServiceNotificationPublisher { notifications })
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

impl ServiceNotificationPublisher for TarpcServiceNotificationPublisher {
    fn send(&self, notification: ServiceNotification) {
        let notifications = self.notifications.clone();
        tokio::spawn(async move {
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
        });
    }
}
