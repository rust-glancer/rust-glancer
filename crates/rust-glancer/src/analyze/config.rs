use clap::ValueEnum;

/// Output format for the `analyze` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum OutputFormat {
    Text,
    Json,
    RichJson,
    Html,
}
