use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use rg_project::{PackageResidencyPolicy, StartupCacheLoad};

mod analyze;
mod runtime;

/// Command-line interface for the `rust-glancer` binary.
#[derive(Debug, Parser)]
#[command(name = "rust-glancer")]
#[command(about = "An incomplete-by-design Rust LSP implementation")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Top-level subcommands supported by the CLI.
#[derive(Debug, Subcommand)]
enum Command {
    /// Analyze the crate or workspace package located at `path`.
    Analyze {
        path: PathBuf,
        /// Print build phase timings after analysis finishes.
        #[clap(long)]
        profile: bool,
        #[clap(short, long)]
        memory: bool,
        /// Load matching offloadable packages from existing cache artifacts during indexing.
        #[clap(short, long)]
        load: bool,
        /// Which packages should remain resident after analysis is built.
        #[clap(long = "package-residency", value_enum, default_value = "all-resident")]
        package_residency: CliPackageResidencyPolicy,
        /// Target triple used to filter Cargo metadata. Defaults to the current rustc host target.
        #[clap(long)]
        target: Option<String>,
    },
    /// Start the language server over stdio.
    Lsp,
}

/// Parses CLI arguments and dispatches to the selected command handler.
fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Analyze {
            path,
            profile,
            memory,
            load,
            package_residency,
            target,
        } => analyze::analyze(
            path,
            profile,
            memory,
            if load {
                StartupCacheLoad::Enabled
            } else {
                StartupCacheLoad::Disabled
            },
            package_residency.into(),
            target,
        ),
        Command::Lsp => rg_lsp::run_stdio_with_memory_control(runtime::memory_control()),
    }
}

/// CLI-facing package residency names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CliPackageResidencyPolicy {
    AllResident,
    Workspace,
    WorkspaceAndPathDeps,
    WorkspacePathAndDirectDeps,
    AllOffloadable,
}

impl From<CliPackageResidencyPolicy> for PackageResidencyPolicy {
    fn from(policy: CliPackageResidencyPolicy) -> Self {
        match policy {
            CliPackageResidencyPolicy::AllResident => Self::AllResident,
            CliPackageResidencyPolicy::Workspace => Self::WorkspaceResident,
            CliPackageResidencyPolicy::WorkspaceAndPathDeps => Self::WorkspaceAndPathDepsResident,
            CliPackageResidencyPolicy::WorkspacePathAndDirectDeps => {
                Self::WorkspacePathAndDirectDepsResident
            }
            CliPackageResidencyPolicy::AllOffloadable => Self::AllOffloadable,
        }
    }
}
