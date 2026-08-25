use std::{
    io::Write as _,
    process::{Command, Stdio},
};

use anyhow::{Context as _, bail};
use rg_parse::LineEndings;
use rg_text::RustEdition;

/// Runs rustfmt as a pure text transformer for LSP formatting.
pub(crate) fn rustfmt(
    text: &str,
    edition: RustEdition,
    line_endings: LineEndings,
) -> anyhow::Result<String> {
    let edition = edition.to_string();
    // Rustfmt's `Auto` style follows the host, while formatting must follow the document.
    let newline_style = match line_endings {
        LineEndings::Lf => "newline_style=Unix",
        LineEndings::Crlf => "newline_style=Windows",
    };
    let mut child = Command::new("rustfmt")
        .args([
            "--emit",
            "stdout",
            "--edition",
            edition.as_str(),
            "--config",
            newline_style,
        ])
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
    use rg_parse::LineIndex;
    use rg_text::RustEdition;

    use super::rustfmt;

    #[test]
    fn formatting_preserves_document_newline_style() {
        let cases = [
            ("LF", "fn main() {\n}\n", "fn main() {}\n"),
            ("CRLF", "fn main() {\r\n}\r\n", "fn main() {}\r\n"),
            ("no newline", "fn main() {}", "fn main() {}\n"),
        ];

        for (case, input, expected) in cases {
            let actual = rustfmt(
                input,
                RustEdition::Edition2024,
                LineIndex::new(input).line_endings(),
            )
            .unwrap_or_else(|error| panic!("rustfmt failed for {case}: {error:#}"));
            assert_eq!(actual, expected, "{case}");
        }
    }

    #[test]
    fn rustfmt_failure_includes_process_context() {
        let input = "pub fn broken(";
        let error = rustfmt(
            input,
            RustEdition::Edition2024,
            LineIndex::new(input).line_endings(),
        )
        .expect_err("invalid Rust should make rustfmt fail");
        let error = format!("{error:#}");

        assert!(
            error.contains("rustfmt failed"),
            "unexpected rustfmt error: {error}"
        );
    }
}
