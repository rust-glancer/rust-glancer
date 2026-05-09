use std::path::PathBuf;

use ls_types::{Diagnostic, NumberOrString};

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
