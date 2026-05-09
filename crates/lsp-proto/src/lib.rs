//! Shared protocol contracts between the Rust Glancer LSP server and analysis engines.
//!
//! This crate owns the request, configuration, and notification types that must be understood on
//! both sides of the LSP/engine boundary. Keeping those contracts here lets the server orchestrate
//! work without depending on engine internals, and lets engine implementations publish results in a
//! common shape.

mod analysis_config;
mod check_config;
mod events;
mod service;

pub use self::{
    analysis_config::AnalysisConfig,
    check_config::CheckConfig,
    events::{EngineEvent, EngineLogLevel},
    service::{EngineNotifyFuture, EngineResultFuture, EngineService, EngineServiceHandle},
};
