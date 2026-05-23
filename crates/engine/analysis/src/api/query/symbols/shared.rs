//! Shared symbol query helpers.

pub(crate) fn field_label(name: Option<String>) -> String {
    name.unwrap_or_else(|| "<unsupported>".to_string())
}
