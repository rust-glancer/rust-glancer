use crate::analyze::report::{ReportAlign, ReportBlock, ReportColumn, ReportRow, ReportValue};

use super::super::value::format_value;
use super::{html_id, render_tab_list, writer::HtmlWriter};

/// Borrowed view of a table block.
///
/// The renderer passes this around instead of unpacking the same long `ReportBlock::Table` pattern
/// in several places.
#[derive(Clone, Copy)]
pub(super) struct TableBlock<'a> {
    key: &'a str,
    title: Option<&'a str>,
    description: Option<&'a str>,
    columns: &'a [ReportColumn],
    rows: &'a [ReportRow],
}

impl<'a> TableBlock<'a> {
    pub(super) fn from_block(block: &'a ReportBlock) -> Option<Self> {
        match block {
            ReportBlock::Table {
                key,
                title,
                description,
                columns,
                rows,
            } => Some(Self {
                key,
                title: title.as_deref(),
                description: description.as_deref(),
                columns,
                rows,
            }),
            _ => None,
        }
    }

    fn panel_id(self, section_key: &str) -> String {
        html_id(section_key, self.key)
    }

    fn tab_title(self) -> &'a str {
        self.title.unwrap_or(self.key)
    }
}

pub(super) fn render_table(html: &mut HtmlWriter, table: TableBlock<'_>) {
    html.element("div")
        .class("report-block")
        .class("table-block")
        .children(|html| {
            if let Some(title) = table.title {
                html.element("h3").text(title);
            }
            if let Some(description) = table.description {
                html.element("p")
                    .class("block-description")
                    .text(description);
            }

            render_table_content(html, table.columns, table.rows);
        });
}

pub(super) fn render_table_tabs(
    html: &mut HtmlWriter,
    section_key: &str,
    tables: &[TableBlock<'_>],
) {
    let set_id = html_id("tables", section_key);
    html.element("div")
        .class("report-block")
        .class("table-tabs")
        .attr("data-tab-set", &set_id)
        .children(|html| {
            render_tab_list(
                html,
                &set_id,
                tables.iter().enumerate().map(|(index, table)| {
                    (
                        index == 0,
                        table.panel_id(section_key),
                        table.tab_title(),
                        table.description,
                    )
                }),
            );

            for (index, table) in tables.iter().enumerate() {
                html.element("div")
                    .attr("id", table.panel_id(section_key))
                    .attr("data-tab-parent", &set_id)
                    .class("tab-panel")
                    .class(if index == 0 { "active" } else { "" })
                    .children(|html| {
                        render_table_content(html, table.columns, table.rows);
                    });
            }
        });
}

fn render_table_content(html: &mut HtmlWriter, columns: &[ReportColumn], rows: &[ReportRow]) {
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
                render_table_header(html, columns);
                render_table_body(html, columns, rows);
            });
        });
    });
}

fn render_table_header(html: &mut HtmlWriter, columns: &[ReportColumn]) {
    html.element("thead").children(|html| {
        html.element("tr").children(|html| {
            html.element("th")
                .attr("scope", "col")
                .attr("data-column-index", "0")
                .class("align-right")
                .class("row-index")
                .class("sortable")
                .text("#");
            for (index, column) in columns.iter().enumerate() {
                html.element("th")
                    .attr("scope", "col")
                    .attr("data-column-index", (index + 1).to_string())
                    .class(align_class(column.align))
                    .class("sortable")
                    .text(&column.title);
            }
        });
    });
}

fn render_table_body(html: &mut HtmlWriter, columns: &[ReportColumn], rows: &[ReportRow]) {
    html.element("tbody").children(|html| {
        for (index, row) in rows.iter().enumerate() {
            html.element("tr").children(|html| {
                html.element("td")
                    .attr("data-sort", index.to_string())
                    .class("align-right")
                    .class("row-index")
                    .text(&(index + 1).to_string());
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

fn align_class(align: ReportAlign) -> &'static str {
    match align {
        ReportAlign::Left => "align-left",
        ReportAlign::Right => "align-right",
        ReportAlign::Center => "align-center",
    }
}
