//! Adapter from indexed source occurrences into analysis cursor symbols.

use rg_ir_model::{TargetRef, identity::DeclarationRef};
use rg_ir_view::source::{
    IndexedSourceFact, IndexedSourceOccurrence, IndexedSourceSurface, IndexedTypePathScope,
};
use rg_parse::{FileId, Span};

use crate::model::{SymbolAt, TypePathScopeRef};

pub(crate) use rg_ir_view::source::IndexedSourceRole as SourceSymbolRole;

/// One source span that can resolve to an analysis symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceSymbol {
    symbol: SymbolAt,
    target: TargetRef,
    file_id: FileId,
    span: Span,
    role: SourceSymbolRole,
    surface: IndexedSourceSurface,
}

impl SourceSymbol {
    pub(crate) fn symbol(&self) -> &SymbolAt {
        &self.symbol
    }

    pub(crate) fn into_symbol(self) -> SymbolAt {
        self.symbol
    }

    pub(crate) fn target(&self) -> TargetRef {
        self.target
    }

    pub(crate) fn file_id(&self) -> FileId {
        self.file_id
    }

    pub(crate) fn span(&self) -> Span {
        self.span
    }

    pub(crate) fn role(&self) -> SourceSymbolRole {
        self.role
    }

    pub(crate) fn surface(&self) -> &IndexedSourceSurface {
        &self.surface
    }

    pub(crate) fn plain_declaration(
        declaration: DeclarationRef,
        target: TargetRef,
        file_id: FileId,
        span: Span,
    ) -> Self {
        Self {
            symbol: SymbolAt::Declaration { declaration, span },
            target,
            file_id,
            span,
            role: SourceSymbolRole::Declaration,
            surface: IndexedSourceSurface::Plain,
        }
    }

    pub(super) fn from_occurrence(occurrence: IndexedSourceOccurrence) -> Self {
        let (fact, target, file_id, span, role, surface) = occurrence.into_parts();
        let symbol = match fact {
            IndexedSourceFact::Declaration(declaration) => {
                SymbolAt::Declaration { declaration, span }
            }
            IndexedSourceFact::FunctionBody(body) => SymbolAt::FunctionBody { body },
            IndexedSourceFact::Expr(expr) => SymbolAt::Expr { expr },
            IndexedSourceFact::TypePath { scope, path } => SymbolAt::TypePath {
                scope: match scope {
                    IndexedTypePathScope::Signature(context) => {
                        TypePathScopeRef::signature(context)
                    }
                    IndexedTypePathScope::Body(scope) => TypePathScopeRef::body(scope),
                },
                path,
                span,
            },
            IndexedSourceFact::ValuePath { scope, path } => {
                SymbolAt::ValuePath { scope, path, span }
            }
            IndexedSourceFact::RecordField { scope, owner, key } => SymbolAt::RecordField {
                scope,
                owner,
                key,
                span,
            },
            IndexedSourceFact::UsePath { module, path } => SymbolAt::UsePath { module, path, span },
        };
        Self {
            symbol,
            target,
            file_id,
            span,
            role,
            surface,
        }
    }
}
