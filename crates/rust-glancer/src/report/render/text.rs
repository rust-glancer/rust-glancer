use std::fmt::Write as _;

use crate::report::{
    ReportAlign, ReportBlock, ReportColumn, ReportDocument, ReportField, ReportRow,
};

use super::value::format_value;

pub(crate) struct TextRenderer;

impl TextRenderer {
    pub(crate) fn render(&self, document: &ReportDocument, out: &mut String) -> std::fmt::Result {
        writeln!(out, "{}", document.title)?;

        for (section_index, section) in document.sections.iter().enumerate() {
            if section_index > 0 {
                writeln!(out)?;
            }

            let block_indent = if let Some(title) = &section.title {
                writeln!(out, "{title}:")?;
                2
            } else {
                0
            };

            if let Some(description) = &section.description {
                writeln!(out, "{}{description}", spaces(block_indent))?;
            }

            for block in &section.blocks {
                self.render_block(block, block_indent, out)?;
            }
        }

        Ok(())
    }

    fn render_block(
        &self,
        block: &ReportBlock,
        indent: usize,
        out: &mut String,
    ) -> std::fmt::Result {
        match block {
            ReportBlock::Paragraph { text } => {
                writeln!(out, "{}{text}", spaces(indent))
            }
            ReportBlock::Fields { title, fields, .. } => {
                self.render_fields(title.as_deref(), fields, indent, out)
            }
            ReportBlock::Table {
                title,
                columns,
                rows,
                ..
            } => self.render_table(title.as_deref(), columns, rows, indent, out),
            ReportBlock::Warning { text } => writeln!(out, "{}warning: {text}", spaces(indent)),
            ReportBlock::Code { text, .. } => {
                for line in text.lines() {
                    writeln!(out, "{}{line}", spaces(indent))?;
                }
                Ok(())
            }
        }
    }

    fn render_fields(
        &self,
        title: Option<&str>,
        fields: &[ReportField],
        indent: usize,
        out: &mut String,
    ) -> std::fmt::Result {
        let field_indent = if let Some(title) = title {
            writeln!(out, "{}{title}:", spaces(indent))?;
            indent + 2
        } else {
            indent
        };

        for field in fields {
            writeln!(
                out,
                "{}{}: {}",
                spaces(field_indent),
                field.title,
                format_value(&field.value),
            )?;
        }

        Ok(())
    }

    fn render_table(
        &self,
        title: Option<&str>,
        columns: &[ReportColumn],
        rows: &[ReportRow],
        indent: usize,
        out: &mut String,
    ) -> std::fmt::Result {
        let table_indent = if let Some(title) = title {
            writeln!(out, "{}{title}:", spaces(indent))?;
            indent + 2
        } else {
            indent
        };

        let widths = column_widths(columns, rows);
        write!(out, "{}", spaces(table_indent))?;
        for (index, column) in columns.iter().enumerate() {
            if index > 0 {
                write!(out, "  ")?;
            }
            write_aligned(out, &column.title, widths[index], column.align)?;
        }
        writeln!(out)?;

        for row in rows {
            write!(out, "{}", spaces(table_indent))?;
            for (index, column) in columns.iter().enumerate() {
                if index > 0 {
                    write!(out, "  ")?;
                }
                let value = row
                    .cells
                    .get(&column.key)
                    .map(format_value)
                    .unwrap_or_else(|| "-".to_string());
                write_aligned(out, &value, widths[index], column.align)?;
            }
            writeln!(out)?;
        }

        Ok(())
    }
}

fn column_widths(columns: &[ReportColumn], rows: &[ReportRow]) -> Vec<usize> {
    columns
        .iter()
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.cells.get(&column.key))
                .map(format_value)
                .map(|value| value.len())
                .chain([column.title.len()])
                .max()
                .unwrap_or(0)
        })
        .collect()
}

fn write_aligned(
    out: &mut String,
    value: &str,
    width: usize,
    align: ReportAlign,
) -> std::fmt::Result {
    match align {
        ReportAlign::Left => write!(out, "{value:<width$}"),
        ReportAlign::Right => write!(out, "{value:>width$}"),
        ReportAlign::Center => {
            let padding = width.saturating_sub(value.len());
            let left = padding / 2;
            let right = padding - left;
            write!(out, "{}{value}{}", spaces(left), spaces(right))
        }
    }
}

fn spaces(count: usize) -> String {
    " ".repeat(count)
}
