//! Shared protocol contracts between the Rust Glancer LSP server and analysis engines.
//!
//! This crate owns the request, configuration, and notification types that must be understood on
//! both sides of the LSP/engine boundary. Keeping those contracts here lets the server orchestrate
//! work without depending on engine internals, and lets engine implementations publish results in a
//! common shape.

mod client_capabilities;
mod completion;
mod config;
mod error;
mod file_uri;
mod notifications;
mod query;
mod saved_source;
mod service;
mod snapshot;

pub use self::{
    client_capabilities::ClientCapabilities,
    completion::CompletionClientCapabilities,
    config::{
        AnalysisCfgConfig, AnalysisConfig, CargoMetadataConfig, CargoMetadataTarget,
        DiagnosticsConfig, EngineConfig, IndexingPerformancePreference, PackageResidencyPolicy,
        SysrootDiscovery,
    },
    error::EngineError,
    file_uri::{FileUriError, file_uri_to_path, path_for_editor, path_to_file_uri},
    notifications::{
        DeferredIndexingOutcome, IndexingProgress, IndexingStage, ServiceLogLevel,
        ServiceNotification,
    },
    query::{QueryError, QueryScope, QueryValue},
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
