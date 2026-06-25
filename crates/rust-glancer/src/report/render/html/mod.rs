//! HTML renderer for analyze reports.
//!
//! The renderer writes one self-contained HTML file. CSS and JavaScript are embedded so the report
//! can be opened directly from disk and still have tabs, filtering, and sortable tables.

mod section;
mod table;
mod writer;

use crate::report::{ReportBlock, ReportDocument, ReportField, ReportSection};

use self::{section::SectionGroup, table::TableBlock, writer::HtmlWriter};
use super::value::format_value;

const META_TAGS: &str = r#"<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
"#;

const STYLE: &str = include_str!("assets/report.css");
const SCRIPT: &str = include_str!("assets/report.js");

pub(crate) struct HtmlRenderer;

impl HtmlRenderer {
    pub(crate) fn render(&self, document: &ReportDocument) -> String {
        let groups = SectionGroup::capture(document);
        let mut html = HtmlWriter::new();
        html.raw("<!doctype html>\n");
        html.element("html").attr("lang", "en").children(|html| {
            html.element("head").children(|html| {
                html.raw(META_TAGS);
                html.element("title").text(&document.title);
                html.raw("<style>\n");
                html.raw(STYLE);
                html.raw("\n</style>\n");
            });
            html.element("body").children(|html| {
                html.element("main").children(|html| {
                    html.element("h1").text(&document.title);
                    self.render_groups(html, &groups);
                });
                html.raw("<script>\n");
                html.raw(SCRIPT);
                html.raw("\n</script>\n");
            });
        });
        html.finish()
    }

    fn render_groups(&self, html: &mut HtmlWriter, groups: &[SectionGroup<'_>]) {
        if groups.len() <= 1 {
            for group in groups {
                self.render_group_panel(html, group);
            }
            return;
        }

        html.element("div")
            .class("root-tabs")
            .attr("data-tab-set", "root")
            .children(|html| {
                render_tab_list(
                    html,
                    "root",
                    groups.iter().enumerate().map(|(index, group)| {
                        (index == 0, group.panel_id(), group.title.as_str(), None)
                    }),
                );

                for (index, group) in groups.iter().enumerate() {
                    let panel_id = group.panel_id();
                    html.element("div")
                        .attr("id", panel_id)
                        .attr("data-tab-parent", "root")
                        .class("tab-panel")
                        .class("group-panel")
                        .class(if index == 0 { "active" } else { "" })
                        .children(|html| self.render_group_panel(html, group));
                }
            });
    }

    fn render_group_panel(&self, html: &mut HtmlWriter, group: &SectionGroup<'_>) {
        let collapsible = group.sections.len() > 1;
        for (index, section) in group.sections.iter().enumerate() {
            self.render_section(html, section, collapsible, index == 0);
        }
    }

    fn render_section(
        &self,
        html: &mut HtmlWriter,
        section: &ReportSection,
        collapsible: bool,
        open: bool,
    ) {
        if collapsible {
            let details = html
                .element("details")
                .attr("id", section.key.as_str())
                .class("report-section");
            let details = if open {
                details.attr("open", "open")
            } else {
                details
            };
            details.children(|html| {
                html.element("summary")
                    .class("section-summary")
                    .text(SectionGroup::section_title(section));
                html.element("div").class("section-body").children(|html| {
                    if let Some(description) = &section.description {
                        html.element("p")
                            .class("section-description")
                            .text(description);
                    }

                    self.render_section_body(html, section);
                });
            });
            return;
        }

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

                self.render_section_body(html, section);
            });
    }

    fn render_section_body(&self, html: &mut HtmlWriter, section: &ReportSection) {
        let table_blocks = section
            .blocks
            .iter()
            .filter_map(TableBlock::from_block)
            .collect::<Vec<_>>();

        if table_blocks.len() <= 1 {
            for block in &section.blocks {
                self.render_block(html, block);
            }
            return;
        }

        for block in &section.blocks {
            if !matches!(block, ReportBlock::Table { .. }) {
                self.render_block(html, block);
            }
        }
        table::render_table_tabs(html, &section.key, &table_blocks);
    }

    fn render_block(&self, html: &mut HtmlWriter, block: &ReportBlock) {
        match block {
            ReportBlock::Paragraph { text } => {
                html.element("p").class("report-block").text(text);
            }
            ReportBlock::Fields { title, fields, .. } => {
                self.render_fields(html, title.as_deref(), fields);
            }
            ReportBlock::Table { .. } => {
                if let Some(table) = TableBlock::from_block(block) {
                    table::render_table(html, table);
                }
            }
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
}

fn render_tab_list<'a>(
    html: &mut HtmlWriter,
    parent: &str,
    tabs: impl IntoIterator<Item = (bool, String, &'a str, Option<&'a str>)>,
) {
    html.element("div")
        .class("tab-list")
        .attr("role", "tablist")
        .children(|html| {
            for (active, target, title, description) in tabs {
                html.element("button")
                    .attr("type", "button")
                    .attr("role", "tab")
                    .attr("aria-selected", if active { "true" } else { "false" })
                    .attr("data-tab-parent", parent)
                    .attr("data-tab-target", target)
                    .class("tab-button")
                    .class(if active { "active" } else { "" })
                    .children(|html| {
                        html.text(title);
                        if let Some(description) = description {
                            html.text(" ");
                            html.element("span")
                                .class("tab-help")
                                .attr("title", description)
                                .attr("aria-label", description)
                                .text("?");
                        }
                    });
            }
        });
}

fn html_id(prefix: &str, key: &str) -> String {
    let mut id = String::from(prefix);
    id.push('-');

    for character in key.chars() {
        if character.is_ascii_alphanumeric() {
            id.push(character.to_ascii_lowercase());
        } else {
            id.push('-');
        }
    }

    id
}
