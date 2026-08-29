use std::{net::SocketAddr, path::PathBuf};

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use rg_project::{PackageBatchSize, StartupCacheLoad};

mod analyze;
mod compare_lsp;
mod logging;
mod memory;
mod report;
mod start_engine;
mod start_server;

/// Command-line interface for the `rust-glancer` binary.
#[derive(Debug, Parser)]
#[command(name = "rust-glancer")]
#[command(about = "An incomplete-by-design Rust LSP implementation")]
#[command(version = rg_lsp_server::VERSION)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Top-level subcommands supported by the CLI.
#[derive(Debug, Subcommand)]
enum Command {
    /// Analyze the crate or workspace package located at `path`.
    #[command(after_help = analyze::profile_groups_help())]
    Analyze {
        path: PathBuf,
        /// Collect comma-separated dynamic profile selectors or aliases.
        #[clap(
            long,
            value_name = "SELECTORS",
            num_args = 0..=1,
            default_missing_value = "default"
        )]
        profile: Option<String>,
        #[clap(short, long)]
        memory: bool,
        /// Load matching offloadable packages from existing cache artifacts during indexing.
        #[clap(short, long)]
        load: bool,
        /// Which packages should remain resident after analysis is built.
        #[clap(long = "package-residency", value_enum, default_value = "all-resident")]
        package_residency: analyze::CliPackageResidencyPolicy,
        /// Which indexing performance trade-off rust-glancer should prioritize.
        #[clap(
            long = "indexing-preference",
            value_enum,
            default_value_t = analyze::CliIndexingPreference::default()
        )]
        indexing_preference: analyze::CliIndexingPreference,
        /// Packages processed together by lower-peak-memory batch indexing.
        #[clap(long, default_value_t = PackageBatchSize::default())]
        package_batch_size: PackageBatchSize,
        /// Target triple used to filter Cargo metadata. Defaults to the current rustc host target.
        #[clap(long)]
        target: Option<String>,
        /// Render the analysis report for humans or CI tooling.
        #[clap(long, value_enum, default_value = "text")]
        format: analyze::OutputFormat,
    },
    /// Compare rust-glancer LSP query behavior against another LSP server.
    CompareLsp {
        fixture: compare_lsp::CliFixture,
        /// Override the fixture root. Defaults to the selected fixture's configured root.
        #[clap(long)]
        path: Option<PathBuf>,
        /// Render the comparison report for humans or CI tooling.
        #[clap(long, value_enum, default_value = "text")]
        format: compare_lsp::OutputFormat,
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
            load,
            package_residency,
            indexing_preference,
            package_batch_size,
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
                indexing_preference.into(),
                package_batch_size,
                target,
                format,
            )
        }
        Command::CompareLsp {
            fixture,
            path,
            format,
        } => {
            logging::init_plain_tracing();
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("while attempting to build LSP comparison Tokio runtime")?;
            runtime.block_on(compare_lsp::run(fixture, path, format))
        }
        Command::Lsp => start_server::start_server(),
        Command::LspEngine {
            engine_addr,
            notifications_addr,
        } => start_engine::start_engine(engine_addr, notifications_addr),
    }
}
