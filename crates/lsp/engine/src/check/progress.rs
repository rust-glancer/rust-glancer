use ls_types::NumberOrString;
use rg_lsp_proto::EngineEvent;

use crate::events::EngineEventSink;

/// Small wrapper around LSP work-done progress for cargo diagnostics.
///
/// Progress is best-effort: if the client rejects token creation, diagnostics still run and publish.
#[derive(Clone, Debug)]
pub(super) struct CheckProgress {
    events: EngineEventSink,
    token: NumberOrString,
}

impl CheckProgress {
    pub(super) fn new(events: EngineEventSink, token: NumberOrString) -> Self {
        Self { events, token }
    }

    pub(super) fn token(&self) -> &NumberOrString {
        &self.token
    }

    pub(super) async fn begin(&self, command: String) {
        self.events.send(EngineEvent::BeginWorkDoneProgress {
            token: self.token.clone(),
            title: "Cargo diagnostics".to_string(),
            message: Some(command),
        });
    }

    pub(super) async fn finish(&self, status: ProgressFinish) {
        self.events.send(EngineEvent::EndWorkDoneProgress {
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
