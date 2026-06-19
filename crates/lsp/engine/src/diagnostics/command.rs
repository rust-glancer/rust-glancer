use std::time::Instant;

use anyhow::Context as _;
use tokio::process::Command;

use super::{DiagnosticsSnapshot, cargo::CargoDiagnostics};

/// Executes the configured cargo diagnostics command for one diagnostics snapshot.
///
/// A non-zero cargo exit can still be useful when rustc emitted JSON diagnostics, so this only
/// returns an error when cargo fails without producing diagnostics we can publish.
pub(super) struct CargoDiagnosticsCommand {
    snapshot: DiagnosticsSnapshot,
}

impl CargoDiagnosticsCommand {
    pub(super) fn new(snapshot: DiagnosticsSnapshot) -> Self {
        Self { snapshot }
    }

    pub(super) async fn run(self) -> anyhow::Result<CargoDiagnostics> {
        let started = Instant::now();
        let source = format!("cargo {}", self.snapshot.config.command);
        tracing::info!(
            generation = self.snapshot.generation,
            trigger = %self.snapshot.trigger,
            command = %self.snapshot.config.user_facing_command(&self.snapshot.analysis),
            "starting cargo diagnostics"
        );

        let mut command = Command::new("cargo");
        let cargo_arguments = self
            .snapshot
            .config
            .cargo_arguments(&self.snapshot.analysis);
        command
            .arg(&self.snapshot.config.command)
            .arg("--message-format=json")
            .current_dir(&self.snapshot.workspace_root)
            .kill_on_drop(true);
        command.args(cargo_arguments);
        for (key, value) in &self.snapshot.config.extra_env {
            command.env(key, value);
        }

        let output = command
            .output()
            .await
            .with_context(|| format!("while attempting to run {}", source))?;
        let diagnostics = CargoDiagnostics::parse(
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
