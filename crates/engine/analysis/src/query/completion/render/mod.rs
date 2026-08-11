//! Reusable construction of final completion rows.
//!
//! Resolvers decide which candidates belong at a site. This module turns those candidates into
//! editor-facing labels, details, documentation, insertion text, and stable sort keys without
//! taking ownership of any completion family.

mod definition;
mod field;
mod function;
mod primitive;
mod sort;
mod synthetic;

use crate::model::CompletionKind;

pub(super) use self::definition::{DefinitionCompletionRenderer, DefinitionCompletionRequest};
pub(super) use self::field::FieldCompletionRenderer;
pub(super) use self::function::{FunctionCompletionRenderer, FunctionCompletionRequest};
pub(super) use self::primitive::PrimitiveTypeCompletionRenderer;
pub(super) use self::sort::{CompletionSortPolicy, CompletionSortPriority};
pub(super) use self::synthetic::{SyntheticCompletionCandidate, SyntheticCompletionRenderer};

/// Controls whether accepting a callable candidate inserts syntax around the completed name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CallCompletionKind {
    /// Insert only the completed name.
    Plain,
    /// Insert ordinary call syntax for this candidate kind.
    Call,
    /// Insert method-call syntax and omit the receiver from function placeholders.
    MethodCall,
}

impl CallCompletionKind {
    pub(super) fn inserts_call_syntax(self) -> bool {
        !matches!(self, Self::Plain)
    }
}

/// Build the short kind/name text used when no full declaration signature is available.
pub(super) fn definition_detail(kind: CompletionKind, label: &str) -> String {
    match kind {
        CompletionKind::Attribute => format!("attribute {label}"),
        CompletionKind::Const => format!("const {label}"),
        CompletionKind::Enum => format!("enum {label}"),
        CompletionKind::EnumVariant => format!("variant {label}"),
        CompletionKind::Field => format!("field {label}"),
        CompletionKind::Function => format!("fn {label}"),
        CompletionKind::InherentMethod | CompletionKind::TraitMethod => format!("method {label}"),
        CompletionKind::Keyword => format!("keyword {label}"),
        CompletionKind::Label => format!("label {label}"),
        CompletionKind::Lifetime => format!("lifetime {label}"),
        CompletionKind::Macro => format!("macro {label}"),
        CompletionKind::Module => format!("mod {label}"),
        CompletionKind::PrimitiveType => format!("primitive type {label}"),
        CompletionKind::Postfix => format!("postfix {label}"),
        CompletionKind::Static => format!("static {label}"),
        CompletionKind::Struct => format!("struct {label}"),
        CompletionKind::Trait => format!("trait {label}"),
        CompletionKind::TypeAlias => format!("type {label}"),
        CompletionKind::TypeParameter => format!("type parameter {label}"),
        CompletionKind::Union => format!("union {label}"),
        CompletionKind::Variable => format!("let {label}"),
        CompletionKind::Value => label.to_string(),
    }
}

/// Escapes plain text before embedding it into an LSP snippet placeholder.
pub(super) fn escape_lsp_snippet_text(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        if matches!(ch, '\\' | '$' | '}') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}
