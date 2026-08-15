//! Shared protocol contracts between the Rust Glancer LSP server and analysis engines.
//!
//! This crate owns the request, configuration, and notification types that must be understood on
//! both sides of the LSP/engine boundary. Keeping those contracts here lets the server orchestrate
//! work without depending on engine internals, and lets engine implementations publish results in a
//! common shape.

mod client_capabilities;
mod completion;
mod config;
mod document_query;
mod error;
mod global_operation;
mod notifications;
mod outcome;
mod saved_source;
mod service;
mod snapshot;

pub use self::{
    client_capabilities::ClientCapabilities,
    completion::{CompletionClientCapabilities, CompletionResult},
    config::{
        AnalysisCfgConfig, AnalysisConfig, CargoMetadataConfig, CargoMetadataTarget,
        DiagnosticsConfig, EngineConfig, IndexingPerformancePreference, PackageResidencyPolicy,
        SysrootDiscovery,
    },
    document_query::{DocumentQueryCoverage, DocumentQueryResult},
    error::EngineError,
    global_operation::GlobalOperationResult,
    notifications::{ServiceLogLevel, ServiceNotification},
    outcome::{AnalysisAbort, AnalysisInput, AnalysisOutcome, AnalysisReady},
    saved_source::{CapturedSourceInput, SaveProposal, SavedProjectChanges},
    service::{
        EngineResult, EngineService, EngineServiceClient, NotificationsService,
        NotificationsServiceClient,
    },
    snapshot::{
        DocumentPositionSnapshot, DocumentRangeSnapshot, DocumentRevision, EditorDocumentSnapshot,
        GlobalPositionSnapshot, OpenDocumentSession, OpenDocumentsRevision, TargetDocumentRevision,
    },
};
