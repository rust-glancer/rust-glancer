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
mod notifications;
mod service;

pub use self::{
    client_capabilities::ClientCapabilities,
    completion::CompletionClientCapabilities,
    config::{
        AnalysisCfgConfig, AnalysisConfig, CargoMetadataConfig, CargoMetadataTarget,
        DiagnosticsConfig, EngineConfig, IndexingPerformancePreference, PackageResidencyPolicy,
        SysrootDiscovery,
    },
    error::EngineError,
    notifications::{ServiceLogLevel, ServiceNotification},
    service::{
        EngineResult, EngineService, EngineServiceClient, NotificationsService,
        NotificationsServiceClient,
    },
};
