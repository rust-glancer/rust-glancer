use std::{net::SocketAddr, path::PathBuf, sync::OnceLock};

use anyhow::Context as _;
use clap::{Parser, Subcommand};

mod analyze;
mod compare_lsp;
mod logging;
mod memory;
mod report;
mod start_engine;
mod start_server;

use self::compare_lsp::CliFixture;

/// Command-line interface for the `rust-glancer` binary.
#[derive(Debug, Parser)]
#[command(name = "rust-glancer")]
#[command(about = "An incomplete-by-design Rust LSP implementation")]
#[command(version = rg_lsp_server::VERSION)]
#[command(long_version = cli_version())]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

fn cli_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    match option_env!("RUST_GLANCER_BUILD_SOURCE") {
        Some(source) => VERSION.get_or_init(|| format!("{} ({source})", rg_lsp_server::VERSION)),
        None => rg_lsp_server::VERSION,
    }
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
        /// Reuse validated results from the saved query cache.
        #[clap(short, long)]
        load: bool,
        /// Target triple used to filter Cargo metadata. Defaults to the current rustc host target.
        #[clap(long)]
        target: Option<String>,
        /// Render the analysis report for humans or CI tooling.
        #[clap(long, value_enum, default_value = "text")]
        format: self::analyze::OutputFormat,
    },
    /// Benchmark editor workloads or run a compatibility sweep.
    CompareLsp {
        fixture: Option<CliFixture>,
        /// Override the fixture root. Defaults to the selected fixture's configured root.
        #[clap(long, requires = "fixture")]
        path: Option<PathBuf>,
        /// Render the comparison report for humans or CI tooling.
        #[clap(long, value_enum)]
        format: Option<self::compare_lsp::OutputFormat>,
        #[command(flatten)]
        benchmark: self::compare_lsp::BenchmarkArgs,
    },
    /// Regenerate checked-in parser and syntax files.
    Codegen {
        #[arg(value_enum, default_value_t = rg_codegen::CodegenCommand::All)]
        command: rg_codegen::CodegenCommand,
        /// Verify generated files without writing changes.
        #[arg(long)]
        check: bool,
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
            target,
            format,
        } => {
            logging::init_plain_tracing();
            analyze::analyze(path, profile, memory, load, target, format)
        }
        Command::CompareLsp {
            fixture,
            path,
            format,
            benchmark,
        } => {
            logging::init_plain_tracing();
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("while attempting to build LSP comparison Tokio runtime")?;
            runtime.block_on(compare_lsp::run(fixture, path, format, benchmark))
        }
        Command::Codegen { command, check } => {
            command.run(check);
            Ok(())
        }
        Command::Lsp => start_server::start_server(),
        Command::LspEngine {
            engine_addr,
            notifications_addr,
        } => start_engine::start_engine(engine_addr, notifications_addr),
    }
}
