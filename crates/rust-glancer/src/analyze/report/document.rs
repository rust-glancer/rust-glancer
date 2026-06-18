use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ReportDocument {
    pub(crate) key: String,
    pub(crate) title: String,
    pub(crate) sections: Vec<ReportSection>,
}

impl ReportDocument {
    pub(crate) fn builder(key: impl Into<String>) -> ReportDocumentBuilder {
        ReportDocumentBuilder::new(key)
    }

    pub(crate) fn new(key: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            sections: Vec::new(),
        }
    }

    pub(crate) fn push_section(&mut self, section: ReportSection) {
        if !section.blocks.is_empty() {
            self.sections.push(section);
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ReportSection {
    pub(crate) key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    pub(crate) blocks: Vec<ReportBlock>,
}

impl ReportSection {
    pub(crate) fn new(key: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            title: Some(title.into()),
            description: None,
            blocks: Vec::new(),
        }
    }

    pub(crate) fn push_block(&mut self, block: ReportBlock) {
        self.blocks.push(block);
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
// The document vocabulary is intentionally wider than the first analyze report producer.
// Renderers should support these shapes before every shape has a data source.
#[allow(dead_code)]
pub(crate) enum ReportBlock {
    Paragraph {
        text: String,
    },
    Fields {
        key: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        fields: Vec<ReportField>,
    },
    Table {
        key: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        columns: Vec<ReportColumn>,
        rows: Vec<ReportRow>,
    },
    Warning {
        text: String,
    },
    Code {
        #[serde(skip_serializing_if = "Option::is_none")]
        language: Option<String>,
        text: String,
    },
}

impl ReportBlock {
    pub(crate) fn fields(
        key: impl Into<String>,
        title: Option<String>,
        fields: Vec<ReportField>,
    ) -> Self {
        Self::Fields {
            key: key.into(),
            title,
            fields,
        }
    }

    pub(crate) fn table(
        key: impl Into<String>,
        title: Option<String>,
        columns: Vec<ReportColumn>,
        rows: Vec<ReportRow>,
    ) -> Self {
        Self::Table {
            key: key.into(),
            title,
            columns,
            rows,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ReportField {
    pub(crate) key: String,
    pub(crate) title: String,
    pub(crate) value: ReportValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
}

impl ReportField {
    pub(crate) fn new(
        key: impl Into<String>,
        title: impl Into<String>,
        value: ReportValue,
    ) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            value,
            description: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ReportColumn {
    pub(crate) key: String,
    pub(crate) title: String,
    pub(crate) align: ReportAlign,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) unit: Option<ReportUnit>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ReportRow {
    pub(crate) cells: BTreeMap<String, ReportValue>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
// Numeric variants keep unit-bearing values distinct in rich JSON instead of folding them into
// strings just because the first text report mostly prints counts, bytes, and durations.
#[allow(dead_code)]
pub(crate) enum ReportValue {
    Text(String),
    Count(u64),
    Integer(i64),
    Float(f64),
    Bool(bool),
    Bytes(u64),
    BytesDelta(i64),
    DurationMs(f64),
    Percent(f64),
    Empty,
}

impl ReportValue {
    pub(crate) fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    pub(crate) fn count(value: usize) -> Self {
        Self::Count(value as u64)
    }

    pub(crate) fn bytes(value: usize) -> Self {
        Self::Bytes(value as u64)
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub(crate) enum ReportAlign {
    Left,
    Right,
    Center,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub(crate) enum ReportUnit {
    Bytes,
    Duration,
    Percent,
    Count,
}

pub(crate) struct ReportDocumentBuilder {
    document: ReportDocument,
}

impl ReportDocumentBuilder {
    fn new(key: impl Into<String>) -> Self {
        let key = key.into();
        Self {
            document: ReportDocument::new(&key, default_title(&key)),
        }
    }

    pub(crate) fn title(mut self, title: impl Into<String>) -> Self {
        self.document.title = title.into();
        self
    }

    pub(crate) fn section<R>(
        mut self,
        key: impl Into<String>,
        configure: impl FnOnce(&mut ReportSectionBuilder) -> R,
    ) -> Self {
        let mut section = ReportSectionBuilder::new(key);
        configure(&mut section);
        self.document.push_section(section.build());
        self
    }

    pub(crate) fn build(self) -> ReportDocument {
        self.document
    }
}

pub(crate) struct ReportSectionBuilder {
    section: ReportSection,
}

impl ReportSectionBuilder {
    fn new(key: impl Into<String>) -> Self {
        let key = key.into();
        Self {
            section: ReportSection::new(&key, default_title(&key)),
        }
    }

    pub(crate) fn title(&mut self, title: impl Into<String>) -> &mut Self {
        self.section.title = Some(title.into());
        self
    }

    pub(crate) fn untitled(&mut self) -> &mut Self {
        self.section.title = None;
        self
    }

    pub(crate) fn fields<R>(
        &mut self,
        key: impl Into<String>,
        configure: impl FnOnce(&mut ReportFieldsBuilder) -> R,
    ) -> &mut Self {
        let mut fields = ReportFieldsBuilder::new(key);
        configure(&mut fields);
        self.section.push_block(fields.build());
        self
    }

    pub(crate) fn table<R>(
        &mut self,
        key: impl Into<String>,
        configure: impl FnOnce(&mut ReportTableBuilder) -> R,
    ) -> &mut Self {
        let mut table = ReportTableBuilder::new(key);
        configure(&mut table);
        self.section.push_block(table.build());
        self
    }

    fn build(self) -> ReportSection {
        self.section
    }
}

pub(crate) struct ReportFieldsBuilder {
    key: String,
    title: Option<String>,
    fields: Vec<ReportField>,
}

impl ReportFieldsBuilder {
    fn new(key: impl Into<String>) -> Self {
        let key = key.into();
        let title = block_title(&key);
        Self {
            key,
            title,
            fields: Vec::new(),
        }
    }

    pub(crate) fn title(&mut self, title: impl Into<String>) -> &mut Self {
        self.title = Some(title.into());
        self
    }

    pub(crate) fn value(&mut self, key: impl Into<String>, value: ReportValue) -> &mut Self {
        let key = key.into();
        self.fields
            .push(ReportField::new(&key, default_title(&key), value));
        self
    }

    pub(crate) fn value_as(
        &mut self,
        key: impl Into<String>,
        title: impl Into<String>,
        value: ReportValue,
    ) -> &mut Self {
        self.fields.push(ReportField::new(key, title, value));
        self
    }

    pub(crate) fn text(&mut self, key: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.value(key, ReportValue::text(value))
    }

    pub(crate) fn count_as(
        &mut self,
        key: impl Into<String>,
        title: impl Into<String>,
        value: usize,
    ) -> &mut Self {
        self.value_as(key, title, ReportValue::count(value))
    }

    pub(crate) fn bytes_as(
        &mut self,
        key: impl Into<String>,
        title: impl Into<String>,
        value: usize,
    ) -> &mut Self {
        self.value_as(key, title, ReportValue::bytes(value))
    }

    fn build(self) -> ReportBlock {
        ReportBlock::fields(self.key, self.title, self.fields)
    }
}

pub(crate) struct ReportTableBuilder {
    key: String,
    title: Option<String>,
    columns: Vec<ReportColumn>,
    rows: Vec<ReportRow>,
}

impl ReportTableBuilder {
    fn new(key: impl Into<String>) -> Self {
        let key = key.into();
        Self {
            title: Some(default_title(&key)),
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

    fn build(self) -> ReportBlock {
        ReportBlock::table(self.key, self.title, self.columns, self.rows)
    }
}

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

fn default_title(key: &str) -> String {
    key.replace('_', " ")
}

fn block_title(key: &str) -> Option<String> {
    (key != "summary").then(|| default_title(key))
}
