//! Shared protocol contracts between the Rust Glancer LSP server and analysis engines.
//!
//! This crate owns the request, configuration, and notification types that must be understood on
//! both sides of the LSP/engine boundary. Keeping those contracts here lets the server orchestrate
//! work without depending on engine internals, and lets engine implementations publish results in a
//! common shape.

mod analysis_config;
mod client_capabilities;
mod completion;
mod diagnostics_config;
mod engine_config;
mod error;
mod notifications;
mod service;

pub use self::{
    analysis_config::{
        AnalysisCfgConfig, AnalysisConfig, CargoMetadataConfig, CargoMetadataTarget,
        IndexingPerformancePreference, PackageResidencyPolicy,
    },
    client_capabilities::ClientCapabilities,
    completion::CompletionClientCapabilities,
    diagnostics_config::DiagnosticsConfig,
    engine_config::EngineConfig,
    error::EngineError,
    notifications::{ServiceLogLevel, ServiceNotification},
    service::{
        EngineResult, EngineService, EngineServiceClient, NotificationsService,
        NotificationsServiceClient,
    },
};
