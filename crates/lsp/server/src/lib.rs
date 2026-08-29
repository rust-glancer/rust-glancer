//! LSP server orchestration for Rust Glancer.
//!
//! This crate adapts the editor-facing LSP transport to Rust Glancer's engine protocol. It owns
//! server capabilities, request routing, client notification forwarding, and engine process
//! orchestration, while keeping analysis implementation details behind engine interfaces.

mod backend;
mod capabilities;
mod client_status;
mod commands;
mod completion_scheduler;
mod config;
mod engine_client;
mod engine_process;
mod engine_registry;
mod file_identity;
mod ingress;
mod inlay_refresher;
mod methods;
mod notifications;
mod project_watcher;
mod recent_editor_saves;
mod stdio;

#[cfg(test)]
mod tests;

/// Version of the published rust-glancer artifacts.
pub use self::methods::SERVER_VERSION as VERSION;
pub use self::stdio::serve_stdio;
