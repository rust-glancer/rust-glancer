//! Analysis cursor symbols built from generic indexed source facts.

use rg_ir_model::TargetRef;
use rg_parse::{FileId, Span};

use crate::{
    api::{
        Analysis,
        view::source::{
            IndexedSourceFact, IndexedSourceOccurrence, IndexedTypePathScope, SourceFactsView,
        },
    },
    model::{SymbolAt, TypePathScopeRef},
};

pub(crate) use crate::api::view::source::IndexedSourceRole as SourceSymbolRole;

/// One source span that can resolve to an analysis symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceSymbol {
    symbol: SymbolAt,
    target: TargetRef,
    file_id: FileId,
    span: Span,
    role: SourceSymbolRole,
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

    fn from_occurrence(occurrence: IndexedSourceOccurrence) -> Self {
        let (fact, target, file_id, span, role) = occurrence.into_parts();
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
            IndexedSourceFact::UsePath { module, path } => SymbolAt::UsePath { module, path, span },
        };
        Self {
            symbol,
            target,
            file_id,
            span,
            role,
        }
    }
}

pub(crate) struct SourceSymbolIndex<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> SourceSymbolIndex<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn symbols_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        Ok(SourceFactsView::new(self.analysis)
            .occurrences_at(target, file_id, offset)?
            .into_iter()
            .map(SourceSymbol::from_occurrence)
            .collect())
    }

    pub(crate) fn symbols_in_target(
        &self,
        target: TargetRef,
        file_id: Option<FileId>,
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        Ok(SourceFactsView::new(self.analysis)
            .occurrences_in_target(target, file_id)?
            .into_iter()
            .map(SourceSymbol::from_occurrence)
            .collect())
    }
}
