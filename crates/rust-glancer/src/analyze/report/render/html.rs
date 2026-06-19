use crate::analyze::report::{
    ReportAlign, ReportBlock, ReportColumn, ReportDocument, ReportField, ReportRow, ReportSection,
    ReportValue,
};

use super::{html_writer::HtmlWriter, value::format_value};

const META_TAGS: &str = r#"<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
"#;

const STYLE: &str = include_str!("assets/report.css");
const SCRIPT: &str = include_str!("assets/report.js");

pub(crate) struct HtmlRenderer;

struct SectionGroup<'a> {
    key: String,
    title: String,
    sections: Vec<&'a ReportSection>,
}

impl HtmlRenderer {
    pub(crate) fn render(&self, document: &ReportDocument) -> String {
        let groups = section_groups(document);
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
                self.render_tab_list(
                    html,
                    "root",
                    groups.iter().enumerate().map(|(index, group)| {
                        (index == 0, group_panel_id(group), group.title.as_str())
                    }),
                );

                for (index, group) in groups.iter().enumerate() {
                    let panel_id = group_panel_id(group);
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
                    .text(section_title(section));
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
            .filter(|block| matches!(block, ReportBlock::Table { .. }))
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
        self.render_table_tabs(html, &section.key, &table_blocks);
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

    fn render_table_tabs(&self, html: &mut HtmlWriter, section_key: &str, blocks: &[&ReportBlock]) {
        let set_id = html_id("tables", section_key);
        html.element("div")
            .class("report-block")
            .class("table-tabs")
            .attr("data-tab-set", &set_id)
            .children(|html| {
                self.render_tab_list(
                    html,
                    &set_id,
                    blocks.iter().enumerate().filter_map(|(index, block)| {
                        table_block_parts(block).map(|(key, title, _, _)| {
                            (index == 0, html_id(section_key, key), title)
                        })
                    }),
                );

                for (index, block) in blocks.iter().enumerate() {
                    let Some((key, _title, columns, rows)) = table_block_parts(block) else {
                        continue;
                    };
                    html.element("div")
                        .attr("id", html_id(section_key, key))
                        .attr("data-tab-parent", &set_id)
                        .class("tab-panel")
                        .class(if index == 0 { "active" } else { "" })
                        .children(|html| {
                            self.render_table_content(html, columns, rows);
                        });
                }
            });
    }

    fn render_tab_list<'a>(
        &self,
        html: &mut HtmlWriter,
        parent: &str,
        tabs: impl IntoIterator<Item = (bool, String, &'a str)>,
    ) {
        html.element("div")
            .class("tab-list")
            .attr("role", "tablist")
            .children(|html| {
                for (active, target, title) in tabs {
                    html.element("button")
                        .attr("type", "button")
                        .attr("role", "tab")
                        .attr("aria-selected", if active { "true" } else { "false" })
                        .attr("data-tab-parent", parent)
                        .attr("data-tab-target", target)
                        .class("tab-button")
                        .class(if active { "active" } else { "" })
                        .text(title);
                }
            });
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

                self.render_table_content(html, columns, rows);
            });
    }

    fn render_table_content(
        &self,
        html: &mut HtmlWriter,
        columns: &[ReportColumn],
        rows: &[ReportRow],
    ) {
        html.element("div").class("table-content").children(|html| {
            if rows.len() > 8 {
                html.element("div").class("table-tools").children(|html| {
                    html.element("input")
                        .class("table-filter")
                        .attr("type", "search")
                        .attr("placeholder", "Filter rows")
                        .attr("aria-label", "Filter table rows")
                        .empty();
                });
            }

            html.element("div").class("table-wrap").children(|html| {
                html.element("table").class("data-table").children(|html| {
                    self.render_table_header(html, columns);
                    self.render_table_body(html, columns, rows);
                });
            });
        });
    }

    fn render_table_header(&self, html: &mut HtmlWriter, columns: &[ReportColumn]) {
        html.element("thead").children(|html| {
            html.element("tr").children(|html| {
                for (index, column) in columns.iter().enumerate() {
                    html.element("th")
                        .attr("scope", "col")
                        .attr("data-column-index", index.to_string())
                        .class(align_class(column.align))
                        .class("sortable")
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
                        let sort_value = row
                            .cells
                            .get(&column.key)
                            .map(cell_sort_value)
                            .unwrap_or_default();

                        html.element("td")
                            .attr("data-sort", sort_value)
                            .class(align_class(column.align))
                            .text(&value);
                    }
                });
            }
        });
    }
}

fn section_groups(document: &ReportDocument) -> Vec<SectionGroup<'_>> {
    let mut groups = Vec::<SectionGroup<'_>>::new();
    for section in &document.sections {
        let (key, title) = section
            .group
            .as_ref()
            .map(|group| (group.key.as_str(), group.title.as_str()))
            .unwrap_or_else(|| (section.key.as_str(), section_title(section)));

        if let Some(group) = groups.iter_mut().find(|group| group.key == key) {
            group.sections.push(section);
        } else {
            groups.push(SectionGroup {
                key: key.to_string(),
                title: title.to_string(),
                sections: vec![section],
            });
        }
    }

    groups
}

fn section_title(section: &ReportSection) -> &str {
    section.title.as_deref().unwrap_or(section.key.as_str())
}

fn group_panel_id(group: &SectionGroup<'_>) -> String {
    html_id("group", &group.key)
}

fn table_block_parts(block: &ReportBlock) -> Option<(&str, &str, &[ReportColumn], &[ReportRow])> {
    match block {
        ReportBlock::Table {
            key,
            title,
            columns,
            rows,
        } => Some((key, title.as_deref().unwrap_or(key.as_str()), columns, rows)),
        _ => None,
    }
}

fn cell_sort_value(value: &ReportValue) -> String {
    match value {
        ReportValue::Text(value) => value.to_lowercase(),
        ReportValue::Count(value) | ReportValue::Bytes(value) => value.to_string(),
        ReportValue::Integer(value) | ReportValue::BytesDelta(value) => value.to_string(),
        ReportValue::Float(value)
        | ReportValue::DurationMs(value)
        | ReportValue::Percent(value) => value.to_string(),
        ReportValue::Bool(value) => {
            if *value {
                "1".to_string()
            } else {
                "0".to_string()
            }
        }
        ReportValue::Empty => String::new(),
    }
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

fn align_class(align: ReportAlign) -> &'static str {
    match align {
        ReportAlign::Left => "align-left",
        ReportAlign::Right => "align-right",
        ReportAlign::Center => "align-center",
    }
}
