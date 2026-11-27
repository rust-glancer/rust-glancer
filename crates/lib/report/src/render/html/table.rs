use super::{html_id, render_tab_list, writer::HtmlWriter};
use crate::{
    ReportAlign, ReportBlock, ReportColumn, ReportRow, ReportValue, render::value::format_value,
};

/// Borrowed view of a table block.
///
/// The renderer passes this around instead of unpacking the same long `ReportBlock::Table` pattern
/// in several places.
#[derive(Clone, Copy)]
pub(super) struct TableBlock<'a> {
    key: &'a str,
    title: Option<&'a str>,
    description: Option<&'a str>,
    page_size: Option<usize>,
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
                page_size,
                columns,
                rows,
            } => Some(Self {
                key,
                title: title.as_deref(),
                description: description.as_deref(),
                page_size: *page_size,
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

            render_table_content(html, table);
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
                        render_table_content(html, *table);
                    });
            }
        });
}

fn render_table_content(html: &mut HtmlWriter, table: TableBlock<'_>) {
    html.element("div").class("table-content").children(|html| {
        if table.rows.len() > 8 || table.columns.iter().any(|column| column.filterable) {
            html.element("div").class("table-tools").children(|html| {
                for (index, column) in table
                    .columns
                    .iter()
                    .enumerate()
                    .filter(|(_, column)| column.filterable)
                {
                    let values = table
                        .rows
                        .iter()
                        .filter_map(|row| row.cells.get(&column.key))
                        .map(format_value)
                        .collect::<std::collections::BTreeSet<_>>();
                    html.element("label")
                        .class("select-filter")
                        .children(|html| {
                            html.text(&column.title);
                            html.element("select")
                                .attr("data-filter-column", (index + 1).to_string())
                                .children(|html| {
                                    html.element("option").attr("value", "").text("All");
                                    for value in values {
                                        html.element("option").attr("value", &value).text(&value);
                                    }
                                });
                        });
                }
                if table.rows.len() > 8 {
                    html.element("input")
                        .class("table-filter")
                        .attr("type", "search")
                        .attr("placeholder", "Filter rows")
                        .attr("aria-label", "Filter table rows")
                        .empty();
                }
            });
        }

        html.element("div").class("table-wrap").children(|html| {
            html.element("table").class("data-table").children(|html| {
                render_table_header(html, table.columns);
                render_table_body(html, table.columns, table.rows);
            });
        });
        if let Some(page_size) = table.page_size {
            html.element("div")
                .class("table-pages")
                .attr("data-page-size", page_size.to_string())
                .children(|html| {
                    html.element("button")
                        .attr("type", "button")
                        .attr("data-page", "prev")
                        .text("Previous");
                    html.element("span")
                        .class("page-summary")
                        .attr("aria-live", "polite")
                        .text("");
                    html.element("button")
                        .attr("type", "button")
                        .attr("data-page", "next")
                        .text("Next");
                });
        }
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
                    .children(|html| {
                        html.text(&column.title);
                        if let Some(help) = &column.help {
                            render_hint(html, help);
                        }
                    });
            }
        });
    });
}

fn render_table_body(html: &mut HtmlWriter, columns: &[ReportColumn], rows: &[ReportRow]) {
    html.element("tbody").children(|html| {
        for (index, row) in rows.iter().enumerate() {
            let tr = html.element("tr");
            let tr = match &row.group {
                Some(group) => tr.attr("data-row-group", group),
                None => tr,
            };
            tr.children(|html| {
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

                    let cell = html
                        .element("td")
                        .attr("data-sort", sort_value)
                        .attr("data-filter-value", &value)
                        .class(align_class(column.align));
                    let cell = if row.highlights.contains(&column.key) {
                        cell.class("metric-highlight")
                    } else {
                        cell
                    };
                    cell.children(|html| {
                        if let Some(details) = row.details.get(&column.key) {
                            html.element("details")
                                .class("cell-details")
                                .children(|html| {
                                    html.element("summary").text(&value);
                                    html.element("pre").text(details);
                                });
                        } else if let Some(ReportValue::Link { url, .. }) =
                            row.cells.get(&column.key)
                            && (url.starts_with("https://") || url.starts_with("http://"))
                        {
                            html.element("a").attr("href", url).text(&value);
                        } else {
                            html.text(&value);
                        }
                        if let Some(hint) = row.hints.get(&column.key) {
                            render_hint(html, hint);
                        }
                    });
                }
            });
        }
    });
}

fn render_hint(html: &mut HtmlWriter, help: &str) {
    html.element("span")
        .class("tab-help")
        .attr("tabindex", "0")
        .attr("aria-label", help)
        .attr("title", help)
        .text("?");
}

fn cell_sort_value(value: &ReportValue) -> String {
    match value {
        ReportValue::Text(value) | ReportValue::Link { label: value, .. } => value.to_lowercase(),
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
