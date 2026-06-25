//! Server-specific command lines and initialization knobs.
//!
//! The comparison should measure query behavior, so each server is started with options that avoid
//! background diagnostics and build work where possible. Keeping these knobs here makes process
//! lifecycle code agnostic to the particular server being spawned.

use std::{env, ffi::OsString};

use anyhow::Context as _;
use serde_json::{Value, json};

const RUST_ANALYZER_ENV: &str = "RUST_GLANCER_COMPARE_LSP_RUST_ANALYZER";

/// Server implementation used for one side of the comparison.
#[derive(Debug, Clone, Copy)]
pub(super) enum ServerKind {
    RustGlancer,
    RustAnalyzer,
}

impl ServerKind {
    pub(super) fn display_name(self) -> &'static str {
        match self {
            Self::RustGlancer => "rust-glancer",
            Self::RustAnalyzer => "rust-analyzer",
        }
    }

    /// Resolve the executable and arguments used to start this server.
    pub(super) fn command_spec(self) -> anyhow::Result<CommandSpec> {
        match self {
            Self::RustGlancer => {
                let executable = env::current_exe()
                    .context("Resolving current rust-glancer executable failed")?;
                Ok(CommandSpec::new(executable, [OsString::from("lsp")]))
            }
            Self::RustAnalyzer => {
                let executable = env::var_os(RUST_ANALYZER_ENV)
                    .unwrap_or_else(|| OsString::from("rust-analyzer"));
                Ok(CommandSpec::new(executable, []))
            }
        }
    }

    /// Disable background work that would make query latency less comparable.
    pub(super) fn initialization_options(self) -> Value {
        match self {
            Self::RustGlancer => json!({
                "diagnostics": {
                    "onStartup": false,
                    "onSave": false,
                },
            }),
            Self::RustAnalyzer => json!({
                "checkOnSave": false,
                "cargo": {
                    "buildScripts": {
                        "enable": false,
                    },
                },
                "diagnostics": {
                    "enable": false,
                },
                "procMacro": {
                    "enable": false,
                },
            }),
        }
    }
}

/// Process command line plus a human-readable label for diagnostics.
#[derive(Debug)]
pub(super) struct CommandSpec {
    pub(super) executable: OsString,
    pub(super) arguments: Vec<OsString>,
}

impl CommandSpec {
    fn new(executable: impl Into<OsString>, arguments: impl IntoIterator<Item = OsString>) -> Self {
        Self {
            executable: executable.into(),
            arguments: arguments.into_iter().collect(),
        }
    }

    pub(super) fn label(&self) -> String {
        let mut parts = vec![self.executable.to_string_lossy().into_owned()];
        parts.extend(
            self.arguments
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned()),
        );
        parts.join(" ")
    }
}
