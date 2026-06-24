use std::{net::SocketAddr, path::PathBuf};

use clap::{Parser, Subcommand};
use rg_project::StartupCacheLoad;

mod analyze;
mod compare_lsp;
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
            compare_lsp::run(fixture, path, format)
        }
        Command::Lsp => start_server::start_server(),
        Command::LspEngine {
            engine_addr,
            notifications_addr,
        } => start_engine::start_engine(engine_addr, notifications_addr),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::{Cli, Command};

    #[test]
    fn analyze_profile_flag_without_value_uses_default_alias() {
        let cli = Cli::try_parse_from(["rust-glancer", "analyze", "/tmp/project", "--profile"])
            .expect("analyze profile flag without a value should parse");
        let Command::Analyze { profile, .. } = cli.command else {
            panic!("CLI should parse the analyze subcommand");
        };

        assert_eq!(
            profile.as_deref(),
            Some("default"),
            "passing --profile without selectors should use the default profile alias",
        );
    }

    #[test]
    fn analyze_without_profile_flag_keeps_profile_disabled() {
        let cli = Cli::try_parse_from(["rust-glancer", "analyze", "/tmp/project"])
            .expect("plain analyze command should parse");
        let Command::Analyze { profile, .. } = cli.command else {
            panic!("CLI should parse the analyze subcommand");
        };

        assert_eq!(
            profile, None,
            "omitting --profile should not implicitly enable profiling",
        );
    }
}
