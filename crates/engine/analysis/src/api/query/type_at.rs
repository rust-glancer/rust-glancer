//! Best-effort type queries over analysis symbols.
//!
//! The public query returns Body IR types because they can describe both semantic items and
//! body-local declarations.

use rg_body_ir::BodyTy;

use crate::{
    api::{Analysis, resolve::declaration::SymbolDeclarationResolver, view::ty::TyView},
    model::SymbolAt,
};

pub(crate) struct TypeResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> TypeResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn type_at(
        &self,
        target: rg_def_map::TargetRef,
        file_id: rg_parse::FileId,
        offset: u32,
    ) -> anyhow::Result<Option<BodyTy>> {
        let Some(symbol) = self.0.symbol_at_for_query(target, file_id, offset)? else {
            return Ok(None);
        };

        let ty = match symbol {
            SymbolAt::Expr { body, expr } => self
                .0
                .body_ir
                .body_data(body)?
                .and_then(|body_data| body_data.expr(expr))
                .map(|data| data.ty.clone()),
            SymbolAt::Binding { body, binding } => self
                .0
                .body_ir
                .body_data(body)?
                .and_then(|body_data| body_data.binding(binding))
                .map(|data| data.ty.clone()),
            SymbolAt::BodyPath {
                body, scope, path, ..
            } => Some(TyView::new(self.0).ty_for_body_type_path(body, scope, &path)?),
            SymbolAt::BodyValuePath {
                body, scope, path, ..
            } => {
                // Value-path type queries should use the same Body IR resolver as the main body
                // pass, so enum variants and associated functions agree between snapshots and
                // cursor-driven editor queries.
                let (_, ty) = self.0.body_ir.resolve_value_path_in_scope(
                    &self.0.def_map,
                    &self.0.semantic_ir,
                    body,
                    scope,
                    &path,
                )?;
                Some(ty)
            }
            declaration_symbol @ (SymbolAt::Def { .. }
            | SymbolAt::Field { .. }
            | SymbolAt::Function { .. }
            | SymbolAt::EnumVariant { .. }
            | SymbolAt::LocalItem { .. }
            | SymbolAt::LocalValueItem { .. }
            | SymbolAt::LocalField { .. }
            | SymbolAt::LocalEnumVariant { .. }
            | SymbolAt::LocalFunction { .. }) => {
                let declarations = SymbolDeclarationResolver::new(self.0)
                    .declarations_for_symbol(declaration_symbol)?;
                let types = TyView::new(self.0);
                let mut ty = None;
                for declaration in declarations {
                    if let Some(declaration_ty) = types.ty_for_declaration(declaration)? {
                        ty = Some(declaration_ty);
                        break;
                    }
                }
                ty
            }
            SymbolAt::TypePath { context, path, .. } => {
                Some(TyView::new(self.0).ty_for_type_path(context, &path)?)
            }
            SymbolAt::UsePath { .. } => None,
            SymbolAt::Body { .. } => None,
        };
        Ok(ty)
    }
}
