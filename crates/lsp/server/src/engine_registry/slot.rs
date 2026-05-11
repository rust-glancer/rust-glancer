use std::{path::PathBuf, sync::Arc};

use tokio::sync::Notify;

use crate::engine_process::EngineProcess;

/// Lifecycle slot for one reserved engine id.
///
/// Routing allocates ids before processes exist, so the registry needs an explicit `Starting` state
/// that other requests can wait on. `Failed` keeps the id occupied and reports the startup error.
#[derive(Debug)]
pub(super) enum EngineSlot {
    Starting { notify: Arc<Notify> },
    Ready(EngineEntry),
    Failed { root: PathBuf, error: Arc<str> },
}

impl EngineSlot {
    pub(super) fn ready(&self) -> Option<&EngineEntry> {
        match self {
            Self::Ready(engine) => Some(engine),
            Self::Starting { .. } | Self::Failed { .. } => None,
        }
    }

    pub(super) fn notify(&self) -> Option<Arc<Notify>> {
        match self {
            Self::Starting { notify, .. } => Some(notify.clone()),
            Self::Ready(_) | Self::Failed { .. } => None,
        }
    }
}

/// Ready engine process plus small lifecycle metadata owned by the server.
#[derive(Debug)]
pub(super) struct EngineEntry {
    pub(super) process: EngineProcess,
}
