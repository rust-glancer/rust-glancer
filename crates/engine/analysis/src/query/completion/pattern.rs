//! Pattern-specific candidate and insertion policy.
//!
//! Source scanning decides which pattern shape surrounds the cursor. This module applies the
//! second half of that decision: whether a semantic candidate can inhabit the shape and, for a
//! bare name, which constructor syntax accepting the candidate should insert.
//!
//! ```text
//! Event::Mes$0          -> Event::Message($0)
//! Event::Mes$0(value)   -> Event::Message(value)
//! Event::Rec$0 { .. }   -> only record-shaped constructors
//! ```
//!
//! Tuple/unit constructors occupy the value namespace, while record constructors are reached
//! through the type namespace. Retaining the constructor shape avoids guessing from labels or
//! signatures when filtering candidates.

use rg_ir_view::{lookup::name::NameNamespace, member::ConstructorShape};

use crate::{
    model::{CompletionInsertText, CompletionKind},
    query::completion::site::PatternCompletionKind,
};

use super::render::escape_lsp_snippet_text;

/// Accepted role for one pattern candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PatternCandidateRole {
    /// Keep the candidate as an ordinary name or path prefix.
    Plain,
    /// Treat the candidate as a constructor and retain its delimiter/field shape.
    Constructor(ConstructorShape),
}

/// Filters candidates and renders constructor syntax for one source pattern shape.
#[derive(Debug, Clone, Copy)]
pub(super) struct PatternCompletionPolicy {
    kind: PatternCompletionKind,
    snippet_support: bool,
}

impl PatternCompletionPolicy {
    pub(super) fn new(kind: PatternCompletionKind, snippet_support: bool) -> Self {
        Self {
            kind,
            snippet_support,
        }
    }

    /// Classify an item by semantic kind and optional constructor shape.
    pub(super) fn candidate(
        self,
        kind: CompletionKind,
        namespace: Option<NameNamespace>,
        constructor: Option<ConstructorShape>,
    ) -> Option<PatternCandidateRole> {
        match self.kind {
            PatternCompletionKind::Name => match kind {
                CompletionKind::Struct | CompletionKind::EnumVariant => {
                    let shape = constructor?;
                    self.constructor_namespace_accepts(namespace, &shape)
                        .then_some(PatternCandidateRole::Constructor(shape))
                }
                // An alias is useful as a qualifier, but Rust does not consistently expose the
                // aliased declaration's constructor through the alias spelling.
                CompletionKind::TypeAlias => Some(PatternCandidateRole::Plain),
                CompletionKind::Module
                | CompletionKind::Enum
                | CompletionKind::Const
                | CompletionKind::Macro => Some(PatternCandidateRole::Plain),
                CompletionKind::Attribute
                | CompletionKind::Field
                | CompletionKind::Function
                | CompletionKind::InherentMethod
                | CompletionKind::Keyword
                | CompletionKind::Label
                | CompletionKind::Lifetime
                | CompletionKind::Postfix
                | CompletionKind::PrimitiveType
                | CompletionKind::Static
                | CompletionKind::Trait
                | CompletionKind::TraitMethod
                | CompletionKind::TypeParameter
                | CompletionKind::Union
                | CompletionKind::Value
                | CompletionKind::Variable => None,
            },
            PatternCompletionKind::TupleConstructor => match constructor {
                Some(shape @ ConstructorShape::Tuple { .. })
                    if matches!(kind, CompletionKind::Struct | CompletionKind::EnumVariant)
                        && self.constructor_namespace_accepts(namespace, &shape) =>
                {
                    Some(PatternCandidateRole::Constructor(shape))
                }
                Some(
                    ConstructorShape::Unit
                    | ConstructorShape::Tuple { .. }
                    | ConstructorShape::Record { .. },
                )
                | None => None,
            },
            PatternCompletionKind::RecordConstructor => match constructor {
                Some(shape @ ConstructorShape::Record { .. })
                    if matches!(kind, CompletionKind::Struct | CompletionKind::EnumVariant)
                        && self.constructor_namespace_accepts(namespace, &shape) =>
                {
                    Some(PatternCandidateRole::Constructor(shape))
                }
                Some(
                    ConstructorShape::Unit
                    | ConstructorShape::Tuple { .. }
                    | ConstructorShape::Record { .. },
                )
                | None => None,
            },
        }
    }

    fn constructor_namespace_accepts(
        self,
        namespace: Option<NameNamespace>,
        shape: &ConstructorShape,
    ) -> bool {
        let Some(namespace) = namespace else {
            return true;
        };
        match shape {
            ConstructorShape::Unit | ConstructorShape::Tuple { .. } => {
                namespace == NameNamespace::Values
            }
            ConstructorShape::Record { .. } => namespace == NameNamespace::Types,
        }
    }

    /// Render text for a constructor selected at this site.
    ///
    /// Explicit tuple/record source already owns its delimiters, so only a bare pattern name adds
    /// them. `plain_is_label` avoids replacing an ordinary unit constructor with redundant custom
    /// text while still supporting qualified expected variants such as `Action::Start`.
    pub(super) fn constructor_insert_text(
        self,
        path: &str,
        shape: &ConstructorShape,
        plain_is_label: bool,
    ) -> CompletionInsertText {
        if !matches!(self.kind, PatternCompletionKind::Name) {
            return CompletionInsertText::Plain;
        }

        match shape {
            ConstructorShape::Unit if plain_is_label => CompletionInsertText::Plain,
            ConstructorShape::Unit => CompletionInsertText::Text(path.to_string()),
            ConstructorShape::Tuple { .. } if self.snippet_support => {
                CompletionInsertText::Snippet(format!("{}($0)", escape_lsp_snippet_text(path)))
            }
            ConstructorShape::Tuple { .. } => CompletionInsertText::Text(format!("{path}()")),
            ConstructorShape::Record { .. } if self.snippet_support => {
                CompletionInsertText::Snippet(format!("{} {{ $0 }}", escape_lsp_snippet_text(path)))
            }
            ConstructorShape::Record { .. } => CompletionInsertText::Text(format!("{path} {{  }}")),
        }
    }
}
