//! Output rendering for the LSP comparison command.

use std::{
    fs,
    io::Write as _,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context as _;

use crate::{
    compare_lsp::{OutputFormat, report::LspComparisonReport},
    report::{self, ReportDocument},
};

pub(crate) fn write_report(
    report: &LspComparisonReport,
    output_format: OutputFormat,
) -> anyhow::Result<()> {
    let output = render_report(report, output_format)?;
    std::io::stdout()
        .lock()
        .write_all(output.as_bytes())
        .context("while attempting to write LSP comparison report")?;

    Ok(())
}

fn render_report(
    report: &LspComparisonReport,
    output_format: OutputFormat,
) -> anyhow::Result<String> {
    match output_format {
        OutputFormat::Text => {
            let document = report.document();
            let mut output = String::new();
            report::TextRenderer
                .render(&document, &mut output)
                .expect("writing to a string should not fail");
            Ok(output)
        }
        OutputFormat::Json => {
            let mut output = report
                .render_json()
                .context("while attempting to render LSP comparison JSON report")?;
            output.push('\n');
            Ok(output)
        }
        OutputFormat::RichJson => {
            let document = report.document();
            let mut output = report::RichJsonRenderer
                .render(&document)
                .context("while attempting to render rich LSP comparison JSON report")?;
            output.push('\n');
            Ok(output)
        }
        OutputFormat::Html => {
            let document = report.document();
            let path = write_html_report(&document)?;
            Ok(format!("wrote HTML report to {}\n", path.display()))
        }
    }
}

fn write_html_report(document: &ReportDocument) -> anyhow::Result<PathBuf> {
    let report_dir = PathBuf::from("target").join("rust_glancer").join("report");
    fs::create_dir_all(&report_dir).with_context(|| {
        format!(
            "while attempting to create HTML report directory {}",
            report_dir.display()
        )
    })?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("while attempting to read system time for HTML report filename")?
        .as_millis();
    let path = report_dir.join(format!("{timestamp}-compare-lsp.html"));
    let html = report::HtmlRenderer.render(document);

    fs::write(&path, html).with_context(|| {
        format!(
            "while attempting to write HTML report file {}",
            path.display()
        )
    })?;

    Ok(path)
}
