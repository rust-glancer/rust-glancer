use std::{
    io::Write as _,
    process::{Command, Stdio},
};

use anyhow::{Context as _, bail};
use rg_workspace::RustEdition;

/// Runs rustfmt as a pure text transformer for LSP formatting.
pub(crate) fn rustfmt(text: &str, edition: RustEdition) -> anyhow::Result<String> {
    let edition = edition.to_string();
    let mut child = Command::new("rustfmt")
        .args(["--emit", "stdout", "--edition", edition.as_str()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("while attempting to spawn rustfmt")?;

    child
        .stdin
        .take()
        .context("while attempting to open rustfmt stdin")?
        .write_all(text.as_bytes())
        .context("while attempting to write source text to rustfmt stdin")?;

    let output = child
        .wait_with_output()
        .context("while attempting to wait for rustfmt")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        if stderr.is_empty() {
            bail!("rustfmt failed with status {}", output.status);
        }
        bail!("rustfmt failed with status {}: {}", output.status, stderr);
    }

    String::from_utf8(output.stdout).context("while attempting to parse rustfmt stdout as UTF-8")
}

#[cfg(test)]
mod tests {
    use rg_workspace::RustEdition;

    use super::rustfmt;

    #[test]
    fn rustfmt_failure_includes_process_context() {
        let error = rustfmt("pub fn broken(", RustEdition::Edition2024)
            .expect_err("invalid Rust should make rustfmt fail");
        let error = format!("{error:#}");

        assert!(
            error.contains("rustfmt failed"),
            "unexpected rustfmt error: {error}"
        );
    }
}
