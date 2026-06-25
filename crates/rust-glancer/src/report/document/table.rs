use std::collections::BTreeMap;

use serde::Serialize;

use super::{
    block::ReportBlock,
    default_title,
    value::{ReportAlign, ReportUnit, ReportValue},
};

/// Table column.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ReportColumn {
    /// Cell key used by rows.
    pub(crate) key: String,
    /// Column label.
    pub(crate) title: String,
    /// How renderers should align this column.
    pub(crate) align: ReportAlign,
    /// Unit used for formatting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) unit: Option<ReportUnit>,
}

/// One table row.
///
/// Rows store cells by column key. Columns decide display order, so row-building code does not need
/// to care about insertion order. Missing cells are allowed.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ReportRow {
    pub(crate) cells: BTreeMap<String, ReportValue>,
}

/// Builder for table blocks.
///
/// Columns are declared before rows so display order is clear, even when rows come from dynamic
/// data.
pub(crate) struct ReportTableBuilder {
    key: String,
    title: Option<String>,
    description: Option<String>,
    columns: Vec<ReportColumn>,
    rows: Vec<ReportRow>,
}

impl ReportTableBuilder {
    pub(super) fn new(key: impl Into<String>) -> Self {
        let key = key.into();
        Self {
            title: Some(default_title(&key)),
            description: None,
            key,
            columns: Vec::new(),
            rows: Vec::new(),
        }
    }

    pub(crate) fn title(&mut self, title: impl Into<String>) -> &mut Self {
        self.title = Some(title.into());
        self
    }

    pub(crate) fn untitled(&mut self) -> &mut Self {
        self.title = None;
        self
    }

    pub(crate) fn description_opt(&mut self, description: Option<String>) -> &mut Self {
        self.description = description;
        self
    }

    pub(crate) fn column(
        &mut self,
        key: impl Into<String>,
        align: ReportAlign,
        unit: Option<ReportUnit>,
    ) -> &mut Self {
        let key = key.into();
        self.columns.push(ReportColumn {
            title: default_title(&key),
            key,
            align,
            unit,
        });
        self
    }

    pub(crate) fn column_as(
        &mut self,
        key: impl Into<String>,
        title: impl Into<String>,
        align: ReportAlign,
        unit: Option<ReportUnit>,
    ) -> &mut Self {
        self.columns.push(ReportColumn {
            key: key.into(),
            title: title.into(),
            align,
            unit,
        });
        self
    }

    pub(crate) fn text_column(&mut self, key: impl Into<String>) -> &mut Self {
        self.column(key, ReportAlign::Left, None)
    }

    pub(crate) fn count_column(&mut self, key: impl Into<String>) -> &mut Self {
        self.column(key, ReportAlign::Right, Some(ReportUnit::Count))
    }

    pub(crate) fn bytes_column(&mut self, key: impl Into<String>) -> &mut Self {
        self.column(key, ReportAlign::Right, Some(ReportUnit::Bytes))
    }

    pub(crate) fn duration_column(&mut self, key: impl Into<String>) -> &mut Self {
        self.column(key, ReportAlign::Right, Some(ReportUnit::Duration))
    }

    pub(crate) fn duration_column_as(
        &mut self,
        key: impl Into<String>,
        title: impl Into<String>,
    ) -> &mut Self {
        self.column_as(key, title, ReportAlign::Right, Some(ReportUnit::Duration))
    }

    pub(crate) fn row<R>(
        &mut self,
        configure: impl FnOnce(&mut ReportRowBuilder) -> R,
    ) -> &mut Self {
        let mut row = ReportRowBuilder::default();
        configure(&mut row);
        self.rows.push(row.build());
        self
    }

    pub(super) fn build(self) -> ReportBlock {
        ReportBlock::table(
            self.key,
            self.title,
            self.description,
            self.columns,
            self.rows,
        )
    }
}

/// Builder for one table row.
#[derive(Default)]
pub(crate) struct ReportRowBuilder {
    cells: BTreeMap<String, ReportValue>,
}

impl ReportRowBuilder {
    pub(crate) fn value(&mut self, key: impl Into<String>, value: ReportValue) -> &mut Self {
        self.cells.insert(key.into(), value);
        self
    }

    pub(crate) fn text(&mut self, key: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.value(key, ReportValue::text(value))
    }

    pub(crate) fn bytes(&mut self, key: impl Into<String>, value: usize) -> &mut Self {
        self.value(key, ReportValue::bytes(value))
    }

    pub(crate) fn duration_ms(&mut self, key: impl Into<String>, value: f64) -> &mut Self {
        self.value(key, ReportValue::DurationMs(value))
    }

    fn build(self) -> ReportRow {
        ReportRow { cells: self.cells }
    }
}
