//! CLI-facing selectors for the LSP comparison command.
//!
//! These enums stay small and human-readable because they are the contract exposed through clap.
//! Fixture resolution and server setup happen in later modules after the user choice is parsed.

use std::fmt as std_fmt;

use clap::ValueEnum;

/// Harmless unsaved edit used by the dirty-buffer comparison fixture.
///
/// The dirty query vector stores coordinates from the pinned checkout and shifts them by this
/// prefix's line count. Keeping the edit independent from Rust syntax lets the report measure
/// source-coordinate handling without also asking either server to understand new declarations.
pub(crate) const DIRTY_EDITOR_PREFIX: &str = "// compare-lsp dirty buffer\n";
pub(crate) const DIRTY_EDITOR_PREFIX_LINE_COUNT: u32 = 1;

/// Golden fixture whose query vector should be compared between servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliFixture {
    #[value(name = "rust_analyzer", alias = "rust-analyzer")]
    RustAnalyzer,
    #[value(name = "rust_analyzer_dirty", alias = "rust-analyzer-dirty")]
    RustAnalyzerDirty,
}

impl CliFixture {
    pub(crate) fn config_name(self) -> &'static str {
        match self {
            Self::RustAnalyzer => "rust_analyzer",
            Self::RustAnalyzerDirty => "rust_analyzer_dirty",
        }
    }

    pub(crate) fn uses_dirty_editor_text(self) -> bool {
        matches!(self, Self::RustAnalyzerDirty)
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
