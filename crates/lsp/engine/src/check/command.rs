use std::time::Instant;

use anyhow::Context as _;
use tokio::process::Command;

use super::{CheckSnapshot, diagnostics::CheckDiagnostics};

/// Executes the configured cargo diagnostics command for one check snapshot.
///
/// A non-zero cargo exit can still be useful when rustc emitted JSON diagnostics, so this only
/// returns an error when cargo fails without producing diagnostics we can publish.
pub(super) struct CargoDiagnosticsCommand {
    snapshot: CheckSnapshot,
}

impl CargoDiagnosticsCommand {
    pub(super) fn new(snapshot: CheckSnapshot) -> Self {
        Self { snapshot }
    }

    pub(super) async fn run(self) -> anyhow::Result<CheckDiagnostics> {
        let started = Instant::now();
        let source = format!("cargo {}", self.snapshot.config.command);
        tracing::info!(
            generation = self.snapshot.generation,
            trigger = %self.snapshot.trigger,
            command = %self.snapshot.config.user_facing_command(),
            "starting cargo diagnostics"
        );

        let mut command = Command::new("cargo");
        command
            .arg(&self.snapshot.config.command)
            .arg("--message-format=json")
            .args(&self.snapshot.config.arguments)
            .current_dir(&self.snapshot.workspace_root)
            .kill_on_drop(true);

        let output = command
            .output()
            .await
            .with_context(|| format!("while attempting to run {}", source))?;
        let diagnostics = CheckDiagnostics::parse(
            &self.snapshot.workspace_root,
            &source,
            &output.stdout,
            &output.stderr,
        );

        if !output.status.success() && diagnostics.is_empty() {
            anyhow::bail!(
                "{} exited with {} and did not produce JSON diagnostics",
                source,
                output.status
            );
        }

        tracing::info!(
            generation = self.snapshot.generation,
            success = output.status.success(),
            diagnostic_files = diagnostics.paths().len(),
            elapsed_ms = started.elapsed().as_millis(),
            "cargo diagnostics finished"
        );

        Ok(diagnostics)
    }
}
