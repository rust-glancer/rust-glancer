use serde::Serialize;

use super::{
    fields::ReportField,
    table::{ReportColumn, ReportRow},
};

/// One block inside a section.
///
/// These are the shapes renderers understand. Keeping the list small means every report is built
/// from the same pieces.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(dead_code)]
pub(crate) enum ReportBlock {
    /// Text paragraph.
    Paragraph { text: String },
    /// Key/value block for short summaries.
    Fields {
        /// Block key.
        key: String,
        /// Block heading.
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// Values shown in order.
        fields: Vec<ReportField>,
    },
    /// Table with columns and rows.
    Table {
        /// Block key.
        key: String,
        /// Table heading.
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// Table note. HTML has room for it; text output can stay terse.
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        /// Column definitions in display order.
        columns: Vec<ReportColumn>,
        /// Data rows in display order.
        rows: Vec<ReportRow>,
    },
    /// Warning text.
    Warning { text: String },
    /// Preformatted code or log text.
    Code {
        /// Language hint for renderers with syntax highlighting.
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
        description: Option<String>,
        columns: Vec<ReportColumn>,
        rows: Vec<ReportRow>,
    ) -> Self {
        Self::Table {
            key: key.into(),
            title,
            description,
            columns,
            rows,
        }
    }
}
