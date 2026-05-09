//! LSP server orchestration for Rust Glancer.
//!
//! This crate adapts the editor-facing LSP transport to Rust Glancer's engine protocol. It owns
//! server capabilities, request routing, client notification forwarding, and stdio startup, while
//! keeping analysis implementation details behind engine interfaces.

mod backend;
mod capabilities;
mod commands;
mod methods;
mod run;

pub use self::run::{run_stdio, run_stdio_with_memory_control};
pub use rg_lsp_engine::{AllocatorPurgeResult, AllocatorStats, MemoryControl};
