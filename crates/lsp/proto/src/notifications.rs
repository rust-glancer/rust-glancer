//! Engine-to-server side effects that are not semantic request responses.
//!
//! Engines do not know about a concrete LSP client. They publish protocol-level progress,
//! diagnostics, refresh, and logging requests here; the server then applies editor currency and
//! presentation policy before forwarding them.

use std::path::PathBuf;

use ls_types::{Diagnostic, NumberOrString};
use serde::{Deserialize, Serialize};

/// Service-originated side effect that the LSP orchestrator should publish to the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServiceNotification {
    PublishDiagnostics {
        path: PathBuf,
        diagnostics: Vec<Diagnostic>,
        /// Exact saved source observed when this Cargo result was prepared for publication.
        ///
        /// The server compares these bytes with an open editor snapshot. `None` covers deleted or
        /// unreadable files and can only be published to a document that is not open.
        saved_text: Option<String>,
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
    DeferredIndexingStarted {
        root: PathBuf,
    },
    DeferredIndexingFinished {
        root: PathBuf,
    },
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
