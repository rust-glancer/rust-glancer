//! Analysis-engine implementation for Rust Glancer LSP sessions.
//!
//! This crate owns workspace analysis, document freshness tracking, diagnostics execution, and
//! memory reporting for an engine instance. Public APIs expose engine construction and event
//! delivery primitives; shared request and notification contracts live in `rg_lsp_proto`.

mod check;
mod documents;
mod engine;
mod events;
mod memory;
mod project_stats;
mod proto;

pub use self::{
    engine::InProcessEngineService,
    events::{EngineEventReceiver, EngineEventSink},
    memory::{AllocatorPurgeResult, AllocatorStats, MemoryControl},
};
