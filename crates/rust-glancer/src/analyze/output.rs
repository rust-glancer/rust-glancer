use std::{
    fs,
    io::Write as _,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context as _;

use super::{
    config::OutputFormat,
    data::{AnalyzeReport, ReportDocumentOptions},
    report::{self, ReportDocument},
};

pub(crate) fn write_report(
    analyze_report: &AnalyzeReport,
    output_format: OutputFormat,
    include_memory: bool,
) -> anyhow::Result<()> {
    let output = render_report(analyze_report, output_format, include_memory)?;
    std::io::stdout()
        .lock()
        .write_all(output.as_bytes())
        .context("while attempting to write analyze report")?;

    Ok(())
}

fn render_report(
    analyze_report: &AnalyzeReport,
    output_format: OutputFormat,
    include_memory: bool,
) -> anyhow::Result<String> {
    match output_format {
        OutputFormat::Text => {
            let mut output = String::new();
            let document = analyze_report.document(document_options(include_memory));
            report::TextRenderer
                .render(&document, &mut output)
                .expect("writing to a string should not fail");
            Ok(output)
        }
        OutputFormat::Json => {
            let mut output = analyze_report
                .render_json()
                .context("while attempting to render analyze JSON report")?;
            output.push('\n');
            Ok(output)
        }
        OutputFormat::RichJson => {
            let document = analyze_report.document(document_options(include_memory));
            let mut output = report::RichJsonRenderer
                .render(&document)
                .context("while attempting to render rich analyze JSON report")?;
            output.push('\n');
            Ok(output)
        }
        OutputFormat::Html => {
            let document = analyze_report.document(document_options(include_memory));
            let path = write_html_report(&document)?;
            Ok(format!("wrote HTML report to {}\n", path.display()))
        }
    }
}

fn document_options(include_memory: bool) -> ReportDocumentOptions {
    ReportDocumentOptions { include_memory }
}

fn write_html_report(document: &ReportDocument) -> anyhow::Result<PathBuf> {
    let report_dir = PathBuf::from("target").join("rust-glancer").join("report");
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
    let path = report_dir.join(format!("{timestamp}-report.html"));
    let html = report::HtmlRenderer.render(document);

    fs::write(&path, html).with_context(|| {
        format!(
            "while attempting to write HTML report file {}",
            path.display()
        )
    })?;

    Ok(path)
}
