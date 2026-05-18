use std::{net::SocketAddr, path::PathBuf};

use clap::{Parser, Subcommand};
use rg_project::StartupCacheLoad;

mod analyze;
mod logging;
mod memory;
mod start_engine;
mod start_server;

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
        /// Build stage used for detailed retained-memory reporting with --memory.
        #[clap(long, value_enum, default_value = "final")]
        stage: analyze::CliMemoryStage,
        /// Load matching offloadable packages from existing cache artifacts during indexing.
        #[clap(short, long)]
        load: bool,
        /// Which packages should remain resident after analysis is built.
        #[clap(long = "package-residency", value_enum, default_value = "all-resident")]
        package_residency: analyze::CliPackageResidencyPolicy,
        /// Target triple used to filter Cargo metadata. Defaults to the current rustc host target.
        #[clap(long)]
        target: Option<String>,
        /// Render the analysis report for humans or CI tooling.
        #[clap(long, value_enum, default_value = "text")]
        format: analyze::OutputFormat,
    },
    /// Start the language server over stdio.
    Lsp,
    /// Start one analysis engine subprocess.
    #[command(hide = true)]
    LspEngine {
        #[clap(long)]
        engine_addr: SocketAddr,
        #[clap(long)]
        notifications_addr: SocketAddr,
    },
}

/// Parses CLI arguments and dispatches to the selected command handler.
fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Analyze {
            path,
            profile,
            memory,
            stage,
            load,
            package_residency,
            target,
            format,
        } => {
            logging::init_plain_tracing();
            analyze::analyze(
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
                format,
                stage,
            )
        }
        Command::Lsp => start_server::start_server(),
        Command::LspEngine {
            engine_addr,
            notifications_addr,
        } => start_engine::start_engine(engine_addr, notifications_addr),
    }
}
