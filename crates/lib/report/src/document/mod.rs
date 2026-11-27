//! Report document model shared by the CLI commands.
//!
//! Report-building code describes data with this IR instead of formatting terminal text or HTML
//! directly. Renderers turn the same sections, fields, tables, and typed values into text, JSON, or
//! HTML.
//!
//! The model stays small on purpose: sections, key/value blocks, tables, warnings, and code
//! snippets. That is enough for these reports without turning this into a layout engine.

mod block;
mod fields;
mod section;
mod table;
mod value;

use serde::Serialize;

pub use self::{
    block::ReportBlock,
    fields::{ReportField, ReportFieldsBuilder},
    section::{ReportSection, ReportSectionBuilder},
    table::{ReportColumn, ReportRow, ReportRowBuilder, ReportTableBuilder},
    value::{ReportAlign, ReportUnit, ReportValue},
};

/// Whole report shared by all renderers.
#[derive(Debug, Clone, Serialize)]
pub struct ReportDocument {
    /// Key for the whole report.
    pub key: String,
    /// Title shown for the report.
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// Non-empty sections in the order report code added them.
    pub sections: Vec<ReportSection>,
}

impl ReportDocument {
    pub fn builder(key: impl Into<String>) -> ReportDocumentBuilder {
        ReportDocumentBuilder::new(key)
    }

    fn new(key: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            subtitle: None,
            sections: Vec::new(),
        }
    }

    fn push_section(&mut self, section: ReportSection) {
        if !section.blocks.is_empty() {
            self.sections.push(section);
        }
    }
}

/// Builder for a report document.
///
/// The builders keep report code readable and produce serializable structs. Renderers do their work
/// later.
pub struct ReportDocumentBuilder {
    document: ReportDocument,
}

impl ReportDocumentBuilder {
    fn new(key: impl Into<String>) -> Self {
        let key = key.into();
        Self {
            document: ReportDocument::new(&key, default_title(&key)),
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.document.title = title.into();
        self
    }

    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.document.subtitle = Some(subtitle.into());
        self
    }

    pub fn section<R>(
        mut self,
        key: impl Into<String>,
        configure: impl FnOnce(&mut ReportSectionBuilder) -> R,
    ) -> Self {
        let mut section = ReportSectionBuilder::new(key);
        configure(&mut section);
        self.document.push_section(section.build());
        self
    }

    pub fn build(self) -> ReportDocument {
        self.document
    }
}

fn default_title(key: &str) -> String {
    key.replace('_', " ")
}

fn block_title(key: &str) -> Option<String> {
    (key != "summary").then(|| default_title(key))
}
