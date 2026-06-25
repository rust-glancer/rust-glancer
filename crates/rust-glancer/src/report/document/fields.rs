use serde::Serialize;

use super::{block::ReportBlock, block_title, default_title, value::ReportValue};

/// One value inside a field block.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ReportField {
    /// Field key.
    pub(crate) key: String,
    /// Label shown for this value.
    pub(crate) title: String,
    /// Typed value. Formatting happens later.
    pub(crate) value: ReportValue,
    /// Extra text for renderers that show descriptions.
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

/// Builder for key/value blocks.
pub(crate) struct ReportFieldsBuilder {
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

    pub(crate) fn value_as_with_description(
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

    pub(super) fn build(self) -> ReportBlock {
        ReportBlock::fields(self.key, self.title, self.fields)
    }
}
