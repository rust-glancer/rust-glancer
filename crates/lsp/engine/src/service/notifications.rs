use rg_lsp_proto::{NotificationsServiceClient, ServiceNotification};

/// Fire-and-forget notifications from the service to the LSP orchestrator.
///
/// Engine and diagnostics work must not block on editor-side publication. Each notification is
/// published on a detached task, keeping progress and diagnostics best-effort across the process
/// boundary.
#[derive(Clone, Debug)]
pub struct ServiceNotificationsSink {
    notifications: NotificationsServiceClient,
}

impl ServiceNotificationsSink {
    pub fn new(notifications: NotificationsServiceClient) -> Self {
        Self { notifications }
    }

    pub(crate) fn send(&self, notification: ServiceNotification) {
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
