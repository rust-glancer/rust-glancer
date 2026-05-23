//! Shared symbol query helpers.

use rg_semantic_ir::TraitData;

pub(crate) fn field_label(name: Option<String>) -> String {
    name.unwrap_or_else(|| "<unsupported>".to_string())
}

pub(crate) fn trait_label(data: &TraitData) -> String {
    format!("trait {}", data.name)
}
