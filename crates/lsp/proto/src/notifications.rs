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
        /// The server compares these bytes with the current open document text. `None` covers
        /// deleted or unreadable files and can only be published to a document that is not open.
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
        generation: u64,
    },
    DeferredIndexingProgress {
        root: PathBuf,
        generation: u64,
        progress: IndexingProgress,
    },
    DeferredIndexingFinished {
        root: PathBuf,
        generation: u64,
        outcome: DeferredIndexingOutcome,
    },
    LogMessage {
        level: ServiceLogLevel,
        message: String,
    },
}

/// Terminal result of deferred work for one queryable saved-project generation.
///
/// Failure does not make the early-start project unusable. It means the background attempt did
/// not finish materializing every deferred payload, so clients can present a degraded-ready state
/// without leaving the indexing operation active forever.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeferredIndexingOutcome {
    Succeeded,
    Failed { message: String },
}

/// Editor-neutral package progress for one indexing stage.
///
/// The server decides how to render this snapshot. Keeping presentation text and LSP tokens out of
/// the engine protocol lets native work-done progress and editor-specific status flows evolve
/// independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexingProgress {
    pub stage: IndexingStage,
    pub completed_packages: u64,
    pub total_packages: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexingStage {
    LoweringBodies,
    ResolvingBodies,
}

/// Client-facing log severity requested by the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceLogLevel {
    Error,
    Warning,
    Info,
    Log,
}
