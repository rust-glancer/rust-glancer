use serde::Serialize;

use super::{block::ReportBlock, block_title, default_title, value::ReportValue};

/// One value inside a field block.
#[derive(Debug, Clone, Serialize)]
pub struct ReportField {
    /// Field key.
    pub key: String,
    /// Label shown for this value.
    pub title: String,
    /// Typed value. Formatting happens later.
    pub value: ReportValue,
    /// Extra text for renderers that show descriptions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl ReportField {
    pub(super) fn new(
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

/// Builder for key/value blocks.
pub struct ReportFieldsBuilder {
    key: String,
    title: Option<String>,
    fields: Vec<ReportField>,
}

impl ReportFieldsBuilder {
    pub(super) fn new(key: impl Into<String>) -> Self {
        let key = key.into();
        let title = block_title(&key);
        Self {
            key,
            title,
            fields: Vec::new(),
        }
    }

    pub fn title(&mut self, title: impl Into<String>) -> &mut Self {
        self.title = Some(title.into());
        self
    }

    pub fn value(&mut self, key: impl Into<String>, value: ReportValue) -> &mut Self {
        let key = key.into();
        self.fields
            .push(ReportField::new(&key, default_title(&key), value));
        self
    }

    pub fn value_as(
        &mut self,
        key: impl Into<String>,
        title: impl Into<String>,
        value: ReportValue,
    ) -> &mut Self {
        self.fields.push(ReportField::new(key, title, value));
        self
    }

    pub fn value_as_with_description(
        &mut self,
        key: impl Into<String>,
        title: impl Into<String>,
        value: ReportValue,
        description: Option<String>,
    ) -> &mut Self {
        let mut field = ReportField::new(key, title, value);
        field.description = description;
        self.fields.push(field);
        self
    }

    pub fn text(&mut self, key: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.value(key, ReportValue::text(value))
    }

    pub fn count_as(
        &mut self,
        key: impl Into<String>,
        title: impl Into<String>,
        value: usize,
    ) -> &mut Self {
        self.value_as(key, title, ReportValue::count(value))
    }

    pub fn bytes_as(
        &mut self,
        key: impl Into<String>,
        title: impl Into<String>,
        value: usize,
    ) -> &mut Self {
        self.value_as(key, title, ReportValue::bytes(value))
    }

    pub(super) fn build(self) -> ReportBlock {
        ReportBlock::fields(self.key, self.title, self.fields)
    }
}
