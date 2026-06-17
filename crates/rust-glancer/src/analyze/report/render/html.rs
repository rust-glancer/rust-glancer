use crate::analyze::report::{
    ReportAlign, ReportBlock, ReportColumn, ReportDocument, ReportField, ReportRow, ReportSection,
};

use super::{html_writer::HtmlWriter, value::format_value};

const META_TAGS: &str = r#"<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
"#;

const STYLE: &str = r#"<style>
:root {
  color-scheme: light;
  --bg: #f7f8fa;
  --surface: #ffffff;
  --text: #1f2933;
  --muted: #667085;
  --border: #d9dee7;
  --accent: #2563eb;
  --warning-bg: #fff7ed;
  --warning-border: #f97316;
}

* {
  box-sizing: border-box;
}

body {
  margin: 0;
  background: var(--bg);
  color: var(--text);
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  font-size: 14px;
  line-height: 1.45;
}

main {
  width: min(1120px, calc(100vw - 32px));
  margin: 32px auto 48px;
}

h1 {
  margin: 0 0 24px;
  font-size: 28px;
  font-weight: 650;
}

h2 {
  margin: 0 0 14px;
  color: var(--accent);
  font-size: 20px;
  font-weight: 650;
}

h3 {
  margin: 0 0 10px;
  font-size: 15px;
  font-weight: 650;
}

.report-section {
  margin: 18px 0;
  padding: 18px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 6px;
}

.section-description {
  margin: 0 0 16px;
  color: var(--muted);
}

.report-block + .report-block {
  margin-top: 18px;
}

.fields {
  display: grid;
  grid-template-columns: minmax(180px, 280px) minmax(0, 1fr);
  gap: 8px 18px;
  margin: 0;
}

.fields dt {
  color: var(--muted);
}

.fields dd {
  margin: 0;
  font-weight: 500;
}

.field-description {
  margin-top: 2px;
  color: var(--muted);
  font-weight: 400;
}

.table-wrap {
  overflow-x: auto;
}

table {
  width: 100%;
  border-collapse: collapse;
  font-variant-numeric: tabular-nums;
}

th,
td {
  padding: 7px 10px;
  border-bottom: 1px solid var(--border);
  vertical-align: top;
  white-space: nowrap;
}

th {
  color: var(--muted);
  font-size: 12px;
  font-weight: 650;
  text-transform: uppercase;
}

tbody tr:nth-child(even) {
  background: #fafbfc;
}

.align-left {
  text-align: left;
}

.align-right {
  text-align: right;
}

.align-center {
  text-align: center;
}

.warning {
  padding: 10px 12px;
  background: var(--warning-bg);
  border-left: 3px solid var(--warning-border);
  border-radius: 4px;
}

pre {
  margin: 0;
  padding: 12px;
  overflow-x: auto;
  background: #111827;
  color: #f9fafb;
  border-radius: 4px;
}

code {
  font-family: "SFMono-Regular", Consolas, "Liberation Mono", monospace;
  font-size: 13px;
}
</style>
"#;

pub(crate) struct HtmlRenderer;

impl HtmlRenderer {
    pub(crate) fn render(&self, document: &ReportDocument) -> String {
        let mut html = HtmlWriter::new();
        html.raw("<!doctype html>\n");
        html.element("html").attr("lang", "en").children(|html| {
            html.element("head").children(|html| {
                html.raw(META_TAGS);
                html.element("title").text(&document.title);
                html.raw(STYLE);
            });
            html.element("body").children(|html| {
                html.element("main").children(|html| {
                    html.element("h1").text(&document.title);
                    for section in &document.sections {
                        self.render_section(html, section);
                    }
                });
            });
        });
        html.finish()
    }

    fn render_section(&self, html: &mut HtmlWriter, section: &ReportSection) {
        html.element("section")
            .attr("id", section.key.as_str())
            .class("report-section")
            .children(|html| {
                if let Some(title) = &section.title {
                    html.element("h2").text(title);
                }

                if let Some(description) = &section.description {
                    html.element("p")
                        .class("section-description")
                        .text(description);
                }

                for block in &section.blocks {
                    self.render_block(html, block);
                }
            });
    }

    fn render_block(&self, html: &mut HtmlWriter, block: &ReportBlock) {
        match block {
            ReportBlock::Paragraph { text } => {
                html.element("p").class("report-block").text(text);
            }
            ReportBlock::Fields { title, fields, .. } => {
                self.render_fields(html, title.as_deref(), fields);
            }
            ReportBlock::Table {
                title,
                columns,
                rows,
                ..
            } => self.render_table(html, title.as_deref(), columns, rows),
            ReportBlock::Warning { text } => {
                html.element("div")
                    .class("report-block")
                    .class("warning")
                    .text(text);
            }
            ReportBlock::Code { language, text } => {
                html.element("pre").class("report-block").children(|html| {
                    let code = html.element("code");
                    let code = if let Some(language) = language {
                        code.class(format!("language-{language}"))
                    } else {
                        code
                    };
                    code.text(text);
                });
            }
        }
    }

    fn render_fields(&self, html: &mut HtmlWriter, title: Option<&str>, fields: &[ReportField]) {
        html.element("div")
            .class("report-block")
            .class("field-block")
            .children(|html| {
                if let Some(title) = title {
                    html.element("h3").text(title);
                }

                html.element("dl").class("fields").children(|html| {
                    for field in fields {
                        html.element("dt").text(&field.title);
                        html.element("dd").children(|html| {
                            html.text(&format_value(&field.value));
                            if let Some(description) = &field.description {
                                html.element("div")
                                    .class("field-description")
                                    .text(description);
                            }
                        });
                    }
                });
            });
    }

    fn render_table(
        &self,
        html: &mut HtmlWriter,
        title: Option<&str>,
        columns: &[ReportColumn],
        rows: &[ReportRow],
    ) {
        html.element("div")
            .class("report-block")
            .class("table-block")
            .children(|html| {
                if let Some(title) = title {
                    html.element("h3").text(title);
                }

                html.element("div").class("table-wrap").children(|html| {
                    html.element("table").children(|html| {
                        self.render_table_header(html, columns);
                        self.render_table_body(html, columns, rows);
                    });
                });
            });
    }

    fn render_table_header(&self, html: &mut HtmlWriter, columns: &[ReportColumn]) {
        html.element("thead").children(|html| {
            html.element("tr").children(|html| {
                for column in columns {
                    html.element("th")
                        .class(align_class(column.align))
                        .text(&column.title);
                }
            });
        });
    }

    fn render_table_body(
        &self,
        html: &mut HtmlWriter,
        columns: &[ReportColumn],
        rows: &[ReportRow],
    ) {
        html.element("tbody").children(|html| {
            for row in rows {
                html.element("tr").children(|html| {
                    for column in columns {
                        let value = row
                            .cells
                            .get(&column.key)
                            .map(format_value)
                            .unwrap_or_else(|| "-".to_string());

                        html.element("td")
                            .class(align_class(column.align))
                            .text(&value);
                    }
                });
            }
        });
    }
}

fn align_class(align: ReportAlign) -> &'static str {
    match align {
        ReportAlign::Left => "align-left",
        ReportAlign::Right => "align-right",
        ReportAlign::Center => "align-center",
    }
}
