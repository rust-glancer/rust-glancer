//! CLI-facing selectors for the LSP comparison command.
//!
//! These enums stay small and human-readable because they are the contract exposed through clap.
//! Fixture resolution and server setup happen in later modules after the user choice is parsed.

use std::fmt as std_fmt;

use clap::ValueEnum;

/// Golden fixture whose query vector should be compared between servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliFixture {
    #[value(name = "rust_analyzer", alias = "rust-analyzer")]
    RustAnalyzer,
}

impl CliFixture {
    pub(crate) fn config_name(self) -> &'static str {
        match self {
            Self::RustAnalyzer => "rust_analyzer",
        }
    }
}

impl std_fmt::Display for CliFixture {
    fn fmt(&self, f: &mut std_fmt::Formatter<'_>) -> std_fmt::Result {
        f.write_str(self.config_name())
    }
}

/// Report format for the `compare-lsp` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum OutputFormat {
    Text,
    Json,
    RichJson,
    Html,
}
