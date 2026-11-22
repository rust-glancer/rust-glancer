use std::collections::{BTreeMap, BTreeSet};

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
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) filterable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) help: Option<String>,
}

/// One table row.
///
/// Rows store cells by column key. Columns decide display order, so row-building code does not need
/// to care about insertion order. Missing cells are allowed.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ReportRow {
    pub(crate) cells: BTreeMap<String, ReportValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) group: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) hints: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) details: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) highlights: BTreeSet<String>,
}

/// Builder for table blocks.
///
/// Columns are declared before rows so display order is clear, even when rows come from dynamic
/// data.
pub(crate) struct ReportTableBuilder {
    key: String,
    title: Option<String>,
    description: Option<String>,
    page_size: Option<usize>,
    columns: Vec<ReportColumn>,
    rows: Vec<ReportRow>,
}

impl ReportTableBuilder {
    pub(super) fn new(key: impl Into<String>) -> Self {
        let key = key.into();
        Self {
            title: Some(default_title(&key)),
            description: None,
            page_size: None,
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
            filterable: false,
            help: None,
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
            filterable: false,
            help: None,
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

    /// Select an exact value or show all values of the named column.
    pub(crate) fn filter_by(&mut self, key: &str) -> &mut Self {
        self.columns
            .iter_mut()
            .find(|column| column.key == key)
            .expect("filter column was declared")
            .filterable = true;
        self
    }

    pub(crate) fn column_help(&mut self, key: &str, text: impl Into<String>) -> &mut Self {
        self.columns
            .iter_mut()
            .find(|column| column.key == key)
            .expect("help column was declared")
            .help = Some(text.into());
        self
    }

    /// Count comparison groups as single entries so pages cannot split a pair.
    pub(crate) fn page_size(&mut self, entries: usize) -> &mut Self {
        self.page_size = Some(entries);
        self
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
            self.page_size,
            self.columns,
            self.rows,
        )
    }
}

/// Builder for one table row.
#[derive(Default)]
pub(crate) struct ReportRowBuilder {
    cells: BTreeMap<String, ReportValue>,
    group: Option<String>,
    hints: BTreeMap<String, String>,
    details: BTreeMap<String, String>,
    highlights: BTreeSet<String>,
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

    /// Keep rows with this group key together when filtering, sorting, or paginating.
    pub(crate) fn group(&mut self, key: impl Into<String>) -> &mut Self {
        self.group = Some(key.into());
        self
    }

    pub(crate) fn hint(&mut self, key: impl Into<String>, text: impl Into<String>) -> &mut Self {
        self.hints.insert(key.into(), text.into());
        self
    }

    pub(crate) fn details(&mut self, key: impl Into<String>, text: impl Into<String>) -> &mut Self {
        self.details.insert(key.into(), text.into());
        self
    }

    pub(crate) fn highlight(&mut self, key: impl Into<String>) -> &mut Self {
        self.highlights.insert(key.into());
        self
    }

    fn build(self) -> ReportRow {
        ReportRow {
            cells: self.cells,
            group: self.group,
            hints: self.hints,
            details: self.details,
            highlights: self.highlights,
        }
    }
}
