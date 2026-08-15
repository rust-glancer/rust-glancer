//! Analysis-engine implementation for Rust Glancer LSP sessions.
//!
//! This crate owns workspace analysis, immutable-input query execution, Cargo diagnostics, and
//! memory reporting for an engine instance. Editor lifecycle and publication currency remain in
//! the LSP server; shared request and notification contracts live in `rg_lsp_proto`.

mod debounce;
mod diagnostics;
mod engine;
mod formatting;
mod memory;
mod project_stats;
mod proto;
mod rpc;
mod service;

#[cfg(test)]
mod tests;

pub use self::{
    memory::{AllocatorStats, MemoryControl},
    rpc::run_rpc,
    service::{Service, ServiceNotificationsSink},
};
