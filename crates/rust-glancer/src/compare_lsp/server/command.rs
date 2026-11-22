//! Server-specific command lines and initialization knobs.
//!
//! The comparison should measure query behavior, so each server is started with options that avoid
//! background diagnostics and build work where possible. Keeping these knobs here makes process
//! lifecycle code agnostic to the particular server being spawned.

use std::{env, ffi::OsString, path::Path};

use anyhow::Context as _;
use serde_json::{Value, json};

const RUST_ANALYZER_ENV: &str = "RUST_GLANCER_COMPARE_LSP_RUST_ANALYZER";

/// Server implementation used for one side of the comparison.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ServerKind {
    RustGlancer,
    RustAnalyzer,
}

impl ServerKind {
    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::RustGlancer => "rust-glancer",
            Self::RustAnalyzer => "rust-analyzer",
        }
    }

    /// Resolve the executable and arguments used to start this server.
    pub(crate) fn command_spec(self, executable: Option<&Path>) -> anyhow::Result<CommandSpec> {
        match self {
            Self::RustGlancer => {
                let executable = match executable {
                    Some(path) => path.to_path_buf(),
                    None => env::current_exe()
                        .context("Resolving current rust-glancer executable failed")?,
                };
                Ok(CommandSpec::new(executable, [OsString::from("lsp")]))
            }
            Self::RustAnalyzer => {
                let executable = executable
                    .map(|path| path.as_os_str().to_owned())
                    .or_else(|| env::var_os(RUST_ANALYZER_ENV))
                    .unwrap_or_else(|| OsString::from("rust-analyzer"));
                Ok(CommandSpec::new(executable, []))
            }
        }
    }

    /// Disable background diagnostics and let the startup workload decide whether to reuse
    /// Rust Glancer's analysis cache. Cargo artifacts can supply generated source to either server.
    pub(crate) fn initialization_options(self, reuse_cache: bool, build_scripts: bool) -> Value {
        match self {
            Self::RustGlancer => json!({
                "cache": {
                    "reuseOnStartup": reuse_cache,
                },
                "diagnostics": {
                    "onStartup": false,
                    "onSave": false,
                },
            }),
            Self::RustAnalyzer => json!({
                "checkOnSave": false,
                "cargo": {
                    "buildScripts": {
                        "enable": build_scripts,
                        "overrideCommand": build_scripts.then_some([
                            "cargo", "check", "--quiet", "--locked", "--offline", "--message-format=json",
                        ]),
                        "useRustcWrapper": !build_scripts,
                    },
                },
                "cachePriming": { "enable": true },
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
pub(crate) struct CommandSpec {
    pub(crate) executable: OsString,
    pub(crate) arguments: Vec<OsString>,
}

impl CommandSpec {
    fn new(executable: impl Into<OsString>, arguments: impl IntoIterator<Item = OsString>) -> Self {
        Self {
            executable: executable.into(),
            arguments: arguments.into_iter().collect(),
        }
    }

    pub(crate) fn label(&self) -> String {
        let mut parts = vec![self.executable.to_string_lossy().into_owned()];
        parts.extend(
            self.arguments
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned()),
        );
        parts.join(" ")
    }
}
