//! Composite implementation lookup over semantic and body-local declarations.
//!
//! Goto-implementation asks for source declarations that implement a selected type, trait, or
//! trait method. This view owns cursor policy and editor projection; reusable impl search lives in
//! the type query layer.

use rg_body_ir::ExprKind;
use rg_ir_model::{
    FunctionRef, ImplRef, SemanticItemRef,
    identity::{DeclarationRef, ExprRef},
};
use rg_ir_storage::ItemStoreQuery;
use rg_ty::{ImplementationQuery, ItemPathQuery, Ty};

use crate::{IndexedViewDb, lookup::resolution::ResolutionView};

pub struct ImplementationView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> ImplementationView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub fn implementations_for_method_call_expr(
        &self,
        expr: ExprRef,
    ) -> anyhow::Result<Option<Vec<DeclarationRef>>> {
        let body = expr.body_ir();
        let Some(body_data) = self.db.body_ir.body_data(body)? else {
            return Ok(None);
        };
        let Some(expr_data) = body_data.expr(expr.expr_id()) else {
            return Ok(None);
        };
        let ExprKind::MethodCall {
            receiver: Some(receiver),
            ..
        } = &expr_data.kind
        else {
            return Ok(None);
        };
        let receiver_ty = body_data.expr(*receiver).map(|data| &data.ty);
        let mut implementations = Vec::new();
        let declarations = ResolutionView::new(self.db)
            .declarations_for_body_resolution(Some(body), &expr_data.resolution)?;
        if declarations.is_empty() {
            return Ok(None);
        }
        let implementation_query = ImplementationQuery::new(ItemPathQuery::new(self.db, self.db));
        for declaration in declarations {
            let Some(function) = self.function_ref_for_declaration(declaration)? else {
                continue;
            };
            Self::extend_function_refs(
                &mut implementations,
                implementation_query.function_implementations(function, receiver_ty)?,
            );
        }
        Ok(Some(implementations))
    }

    pub fn implementations_for_declaration(
        &self,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let mut implementations = Vec::new();
        let implementation_query = ImplementationQuery::new(ItemPathQuery::new(self.db, self.db));

        match declaration {
            DeclarationRef::Item(item) => match item {
                SemanticItemRef::TypeDef(ty) => {
                    Self::extend_impl_refs(
                        &mut implementations,
                        implementation_query.impls_for_type_def(ty)?,
                    );
                }
                SemanticItemRef::Trait(trait_ref) => {
                    Self::extend_impl_refs(
                        &mut implementations,
                        implementation_query.impls_for_trait(trait_ref)?,
                    );
                }
                SemanticItemRef::Function(function) => {
                    Self::extend_function_refs(
                        &mut implementations,
                        implementation_query.function_implementations(function, None)?,
                    );
                }
                SemanticItemRef::Impl(_)
                | SemanticItemRef::TypeAlias(_)
                | SemanticItemRef::Const(_)
                | SemanticItemRef::Static(_) => {}
            },
            DeclarationRef::LocalDef(local_def) => {
                let Some(function) =
                    self.function_ref_for_declaration(DeclarationRef::local_def(local_def))?
                else {
                    return Ok(implementations);
                };
                Self::extend_function_refs(
                    &mut implementations,
                    implementation_query.function_implementations(function, None)?,
                );
            }
            DeclarationRef::BodyBinding(binding) => {
                let Some(body) = self.db.body_ir.body_data(binding.body)? else {
                    return Ok(implementations);
                };
                let Some(binding_data) = body.binding(binding.binding) else {
                    return Ok(implementations);
                };
                Self::extend_impl_refs(
                    &mut implementations,
                    implementation_query.impls_for_ty(&binding_data.ty)?,
                );
            }
            DeclarationRef::Module(_)
            | DeclarationRef::Field(_)
            | DeclarationRef::EnumVariant(_) => {}
        }

        Ok(implementations)
    }

    pub fn implementations_for_ty(&self, ty: &Ty) -> anyhow::Result<Vec<DeclarationRef>> {
        let mut implementations = Vec::new();
        let implementation_query = ImplementationQuery::new(ItemPathQuery::new(self.db, self.db));
        Self::extend_impl_refs(&mut implementations, implementation_query.impls_for_ty(ty)?);
        Ok(implementations)
    }

    fn function_ref_for_declaration(
        &self,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Option<FunctionRef>> {
        match declaration {
            DeclarationRef::LocalDef(local_def) => Ok(ItemStoreQuery::new(self.db)
                .semantic_item_for_local_def(local_def)?
                .and_then(|item| match item {
                    SemanticItemRef::Function(function) => Some(function),
                    _ => None,
                })),
            DeclarationRef::Item(SemanticItemRef::Function(function)) => Ok(Some(function)),
            DeclarationRef::Module(_)
            | DeclarationRef::Item(_)
            | DeclarationRef::Field(_)
            | DeclarationRef::EnumVariant(_)
            | DeclarationRef::BodyBinding(_) => Ok(None),
        }
    }

    fn extend_impl_refs(implementations: &mut Vec<DeclarationRef>, impls: Vec<ImplRef>) {
        for impl_ref in impls {
            Self::push_unique(implementations, DeclarationRef::from(impl_ref));
        }
    }

    fn extend_function_refs(
        implementations: &mut Vec<DeclarationRef>,
        functions: Vec<FunctionRef>,
    ) {
        for function in functions {
            Self::push_unique(implementations, DeclarationRef::from(function));
        }
    }

    fn push_unique(implementations: &mut Vec<DeclarationRef>, declaration: DeclarationRef) {
        if !implementations.contains(&declaration) {
            implementations.push(declaration);
        }
    }
}
