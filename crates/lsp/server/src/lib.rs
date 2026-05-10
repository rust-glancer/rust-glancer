//! LSP server orchestration for Rust Glancer.
//!
//! This crate adapts the editor-facing LSP transport to Rust Glancer's engine protocol. It owns
//! server capabilities, request routing, client notification forwarding, and engine process
//! orchestration, while keeping analysis implementation details behind engine interfaces.

mod backend;
mod capabilities;
mod commands;
mod engine_client;
mod engine_process;
mod engine_registry;
mod methods;
mod notifications;
mod stdio;

#[cfg(test)]
mod tests;

pub use self::stdio::serve_stdio;
