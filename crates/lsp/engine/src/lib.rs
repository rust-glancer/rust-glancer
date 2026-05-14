//! Analysis-engine implementation for Rust Glancer LSP sessions.
//!
//! This crate owns workspace analysis, document freshness tracking, diagnostics execution, and
//! memory reporting for an engine instance. Public APIs expose engine construction and event
//! delivery primitives; shared request and notification contracts live in `rg_lsp_proto`.

mod diagnostics;
mod dirty_state;
mod documents;
mod engine;
mod memory;
mod project_stats;
mod proto;
mod rpc;
mod service;

pub use self::{
    memory::{AllocatorPurgeResult, AllocatorStats, MemoryControl},
    rpc::run_rpc,
    service::{Service, ServiceNotificationsSink},
};
