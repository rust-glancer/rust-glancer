use ls_types::NumberOrString;
use rg_lsp_proto::ServiceNotification;

use crate::service::ServiceNotificationsSink;

/// Small wrapper around LSP work-done progress for cargo diagnostics.
///
/// Progress is best-effort: if the client rejects token creation, diagnostics still run and publish.
#[derive(Clone, Debug)]
pub(super) struct DiagnosticsProgress {
    notifications: ServiceNotificationsSink,
    token: NumberOrString,
}

impl DiagnosticsProgress {
    pub(super) fn new(notifications: ServiceNotificationsSink, token: NumberOrString) -> Self {
        Self {
            notifications,
            token,
        }
    }

    pub(super) fn token(&self) -> &NumberOrString {
        &self.token
    }

    pub(super) async fn begin(&self, command: String) {
        self.notifications
            .send(ServiceNotification::BeginWorkDoneProgress {
                token: self.token.clone(),
                title: "Cargo diagnostics".to_string(),
                message: Some(command),
            });
    }

    pub(super) async fn finish(&self, status: ProgressFinish) {
        self.notifications
            .send(ServiceNotification::EndWorkDoneProgress {
                token: self.token.clone(),
                message: Some(status.message().to_string()),
            });
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ProgressFinish {
    Cancelled,
    Failed,
    Finished,
    Superseded,
}

impl ProgressFinish {
    fn message(self) -> &'static str {
        match self {
            Self::Cancelled => "Cancelled",
            Self::Failed => "Failed",
            Self::Finished => "Finished",
            Self::Superseded => "Superseded",
        }
    }
}
