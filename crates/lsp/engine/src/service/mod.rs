mod notifications;
mod service_impl;

use std::sync::Arc;

use tokio::sync::Mutex;

pub use self::notifications::ServiceNotificationsSink;
use crate::{
    diagnostics::DiagnosticsHandle, documents::DocumentStore, engine::EngineHandle,
    memory::MemoryControl,
};

/// RPC-facing façade owned by one engine process.
///
/// `Service` is the boundary visible to the LSP server: it accepts editor-shaped requests and
/// coordinates analysis, document freshness, and cargo diagnostics without exposing those internal
/// subsystems across the process boundary.
#[derive(Clone, Debug)]
pub struct Service {
    engine: EngineHandle,
    diagnostics: DiagnosticsHandle,
}

impl Service {
    pub fn spawn(
        memory_control: Arc<dyn MemoryControl>,
        notifications: ServiceNotificationsSink,
    ) -> Self {
        let documents = Arc::new(Mutex::new(DocumentStore::default()));
        let engine = EngineHandle::spawn(
            memory_control,
            notifications.clone(),
            Arc::clone(&documents),
        );
        let diagnostics = DiagnosticsHandle::new(notifications, documents);

        Self {
            engine,
            diagnostics,
        }
    }
}
