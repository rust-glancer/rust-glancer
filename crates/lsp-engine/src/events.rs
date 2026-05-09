use rg_lsp_proto::EngineEvent;
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
