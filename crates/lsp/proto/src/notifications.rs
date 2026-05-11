use std::path::PathBuf;

use ls_types::{Diagnostic, NumberOrString};
use serde::{Deserialize, Serialize};

/// Service-originated side effect that the LSP orchestrator should publish to the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServiceNotification {
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
        level: ServiceLogLevel,
        message: String,
    },
}

/// Client-facing log severity requested by the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceLogLevel {
    Error,
    Warning,
    Info,
    Log,
}
