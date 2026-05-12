//! Shared symbol query helpers.

use rg_body_ir::BodyImplData;
use rg_def_map::{ModuleData, ModuleOrigin};
use rg_parse::{FileId, Span};
use rg_semantic_ir::{ImplData, TraitData};

pub(crate) struct SymbolSource {
    pub(crate) span: Span,
    pub(crate) selection_span: Span,
}

pub(crate) struct SymbolSourceWithFile {
    pub(crate) file_id: FileId,
    pub(crate) span: Span,
    pub(crate) selection_span: Span,
}

pub(crate) fn module_declaration_source(module: &ModuleData) -> Option<SymbolSourceWithFile> {
    let (file_id, span) = match module.origin {
        ModuleOrigin::Root { .. } => return None,
        ModuleOrigin::Inline {
            declaration_file,
            declaration_span,
        }
        | ModuleOrigin::OutOfLine {
            declaration_file,
            declaration_span,
            ..
        } => (declaration_file, declaration_span),
    };

    Some(SymbolSourceWithFile {
        file_id,
        span,
        selection_span: module.name_span.unwrap_or(span),
    })
}

pub(crate) fn field_label(name: Option<String>) -> String {
    name.unwrap_or_else(|| "<unsupported>".to_string())
}

pub(crate) fn trait_label(data: &TraitData) -> String {
    format!("trait {}", data.name)
}

pub(crate) fn impl_label(data: &ImplData) -> String {
    match &data.trait_ref {
        Some(trait_ref) => format!("impl {trait_ref} for {}", data.self_ty),
        None => format!("impl {}", data.self_ty),
    }
}

pub(crate) fn body_impl_label(data: &BodyImplData) -> String {
    match &data.trait_ref {
        Some(trait_ref) => format!("impl {trait_ref} for {}", data.self_ty),
        None => format!("impl {}", data.self_ty),
    }
}
