use std::path::PathBuf;

use ls_types::{Diagnostic, NumberOrString};
use tokio::sync::mpsc;

pub type EngineEventReceiver = mpsc::UnboundedReceiver<EngineEvent>;

/// Fire-and-forget event channel from one engine to the LSP orchestrator.
///
/// Engine work must not block on editor-side publication. Keeping this as a queue also mirrors the
/// future subprocess shape, where diagnostics, progress, and logs will cross an RPC boundary.
#[derive(Clone, Debug)]
pub struct EngineEventSink {
    sender: mpsc::UnboundedSender<EngineEvent>,
}

impl EngineEventSink {
    pub fn channel() -> (Self, EngineEventReceiver) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, receiver)
    }

    pub(crate) fn send(&self, event: EngineEvent) {
        if self.sender.send(event).is_err() {
            tracing::debug!("dropped engine event because LSP receiver is closed");
        }
    }
}

/// Engine-originated side effect that the LSP orchestrator should publish to the client.
#[derive(Debug)]
pub enum EngineEvent {
    PublishDiagnostics {
        path: PathBuf,
        diagnostics: Vec<Diagnostic>,
        version: Option<i32>,
    },
    BeginWorkDoneProgress {
        token: NumberOrString,
        title: String,
        message: Option<String>,
    },
    EndWorkDoneProgress {
        token: NumberOrString,
        message: Option<String>,
    },
    InlayHintRefresh,
    LogMessage {
        level: EngineLogLevel,
        message: String,
    },
}

/// Client-facing log severity requested by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineLogLevel {
    Error,
    Warning,
    Info,
    Log,
}
