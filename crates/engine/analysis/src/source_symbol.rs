//! Analysis cursor symbols built from generic indexed source facts.

use rg_ir_model::{TargetRef, identity::DeclarationRef};
use rg_parse::{FileId, Span};
use rg_ty::Ty;

use rg_ir_view::{
    IndexedViewDb,
    lookup::resolution::ResolutionView,
    source::{
        IndexedSourceFact, IndexedSourceOccurrence, IndexedTypePathScope, SourceOccurrenceView,
    },
    ty::TyView,
};

use crate::model::{SymbolAt, TypePathScopeRef, TypePathScopeRepr};

pub(crate) use rg_ir_view::source::IndexedSourceRole as SourceSymbolRole;

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
        }
    }
}

pub(crate) struct SourceSymbolIndex<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> SourceSymbolIndex<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub(crate) fn symbols_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        Ok(SourceOccurrenceView::new(self.db)
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
        Ok(SourceOccurrenceView::new(self.db)
            .occurrences_in_target(target, file_id)?
            .into_iter()
            .map(SourceSymbol::from_occurrence)
            .collect())
    }
}

pub(crate) struct SourceSymbolResolver<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> SourceSymbolResolver<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub(crate) fn declarations_for_symbol(
        &self,
        symbol: SymbolAt,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let resolution = ResolutionView::new(self.db);
        match symbol {
            SymbolAt::FunctionBody { .. } => Ok(Vec::new()),
            SymbolAt::Declaration { declaration, .. } => {
                Ok(vec![resolution.canonical_declaration(declaration)?])
            }
            SymbolAt::Expr { expr } => resolution.declarations_for_expr(expr),
            SymbolAt::TypePath { scope, path, .. } => match scope.repr() {
                TypePathScopeRepr::Signature(context) => {
                    let declarations =
                        resolution.declarations_for_semantic_type_path(context, &path)?;
                    if declarations.is_empty() {
                        resolution.declarations_for_use_path(context.module, &path)
                    } else {
                        Ok(declarations)
                    }
                }
                TypePathScopeRepr::Body(scope) => resolution.declarations_for_body_type_path(
                    scope.body_ir(),
                    scope.scope_id(),
                    &path,
                ),
            },
            SymbolAt::ValuePath { scope, path, .. } => resolution.declarations_for_body_value_path(
                scope.body_ir(),
                scope.scope_id(),
                &path,
            ),
            SymbolAt::RecordField {
                scope, owner, key, ..
            } => resolution.declarations_for_body_record_field(
                scope.body_ir(),
                scope.scope_id(),
                &owner,
                &key,
            ),
            SymbolAt::UsePath { module, path, .. } => {
                resolution.declarations_for_use_path(module, &path)
            }
        }
    }

    pub(crate) fn ty_for_symbol(&self, symbol: SymbolAt) -> anyhow::Result<Option<Ty>> {
        let ty_view = TyView::new(self.db);
        let ty = match symbol {
            SymbolAt::Expr { expr } => ty_view.ty_for_expr(expr)?,
            SymbolAt::Declaration { declaration, .. } => {
                let declaration =
                    ResolutionView::new(self.db).canonical_declaration(declaration)?;
                ty_view.ty_for_declaration(declaration)?
            }
            SymbolAt::TypePath { scope, path, .. } => match scope.repr() {
                TypePathScopeRepr::Signature(context) => {
                    Some(ty_view.ty_for_type_path(context, &path)?)
                }
                TypePathScopeRepr::Body(scope) => {
                    Some(ty_view.ty_for_body_type_path(scope.body_ir(), scope.scope_id(), &path)?)
                }
            },
            SymbolAt::ValuePath { scope, path, .. } => {
                Some(ty_view.ty_for_body_value_path(scope.body_ir(), scope.scope_id(), &path)?)
            }
            SymbolAt::RecordField { .. }
            | SymbolAt::UsePath { .. }
            | SymbolAt::FunctionBody { .. } => None,
        };
        Ok(ty)
    }
}
