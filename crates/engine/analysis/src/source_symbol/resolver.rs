//! Resolution of analysis cursor symbols to declarations and types.

use rg_ir_model::identity::DeclarationRef;
use rg_ir_view::{IndexedViewDb, lookup::resolution::ResolutionView, ty::TyView};
use rg_ty::Ty;

use crate::model::SymbolAt;

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
            SymbolAt::TypePath { scope, path, .. } => {
                resolution.declarations_for_type_path(scope, &path)
            }
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
            SymbolAt::TypePath { scope, path, .. } => {
                Some(ty_view.ty_for_indexed_type_path(scope, &path)?)
            }
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
