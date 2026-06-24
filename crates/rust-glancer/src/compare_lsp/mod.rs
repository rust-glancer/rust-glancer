mod config;

use std::path::PathBuf;

pub(crate) use self::config::{CliFixture, OutputFormat};

pub(crate) fn run(
    fixture: CliFixture,
    path: Option<PathBuf>,
    output_format: OutputFormat,
) -> anyhow::Result<()> {
    let path = path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "<fixture default>".to_string());
    anyhow::bail!(
        "LSP comparison fixture `{fixture}` is not implemented yet \
         (path: {path}, format: {output_format:?})",
    );
}
