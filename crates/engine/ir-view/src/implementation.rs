//! Implementation lookup over indexed views.
//!
//! Goto-implementation is an editor query, but the lookup itself needs direct access to target item
//! indexes and body expression facts. This view keeps those storage-shaped queries out of analysis.

use rg_ir_model::{
    ExprKind, FunctionRef, SemanticItemRef, TargetRef,
    identity::{DeclarationRef, ExprRef},
};
use rg_ir_storage::{ItemStoreQuery, TargetItemQuery};
use rg_ty::{ImplementationQuery, ItemPathQuery, Ty};

use crate::{IndexedViewDb, lookup::resolution::ResolutionView};

pub struct ImplementationView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> ImplementationView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub fn method_call_implementations(
        &self,
        expr: ExprRef,
    ) -> anyhow::Result<Option<Vec<DeclarationRef>>> {
        let body_ref = expr.body_ir();
        let Some(body_data) = self.db.body_ir.body_data(body_ref)? else {
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
        let receiver_ty = body_data.expr_ty(*receiver);
        let declarations = ResolutionView::new(self.db).declarations_for_expr(expr)?;
        if declarations.is_empty() {
            return Ok(None);
        }

        let implementation_query = self.implementation_query(body_ref.target);
        let mut implementations = Vec::new();
        for declaration in declarations {
            let Some(function) = self.function_ref_for_declaration(declaration)? else {
                continue;
            };
            for implementation in
                implementation_query.function_implementations(function, receiver_ty)?
            {
                Self::push_unique(&mut implementations, DeclarationRef::from(implementation));
            }
        }
        Ok(Some(implementations))
    }

    pub fn implementations_for_declaration(
        &self,
        use_site: TargetRef,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let implementation_query = self.implementation_query(use_site);
        let mut implementations = Vec::new();

        match declaration {
            DeclarationRef::Item(item) => match item {
                SemanticItemRef::TypeDef(ty) => {
                    for implementation in implementation_query.impls_for_type_def(ty)? {
                        Self::push_unique(
                            &mut implementations,
                            DeclarationRef::from(implementation),
                        );
                    }
                }
                SemanticItemRef::Trait(trait_ref) => {
                    for implementation in implementation_query.impls_for_trait(trait_ref)? {
                        Self::push_unique(
                            &mut implementations,
                            DeclarationRef::from(implementation),
                        );
                    }
                }
                SemanticItemRef::Function(function) => {
                    for implementation in
                        implementation_query.function_implementations(function, None)?
                    {
                        Self::push_unique(
                            &mut implementations,
                            DeclarationRef::from(implementation),
                        );
                    }
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
                for implementation in
                    implementation_query.function_implementations(function, None)?
                {
                    Self::push_unique(&mut implementations, DeclarationRef::from(implementation));
                }
            }
            DeclarationRef::BodyBinding(binding) => {
                let Some(body) = self.db.body_ir.body_data(binding.body)? else {
                    return Ok(implementations);
                };
                let Some(binding_ty) = body.binding_ty(binding.binding) else {
                    return Ok(implementations);
                };
                for implementation in implementation_query.impls_for_ty(binding_ty)? {
                    Self::push_unique(&mut implementations, DeclarationRef::from(implementation));
                }
            }
            DeclarationRef::Module(_)
            | DeclarationRef::Field(_)
            | DeclarationRef::EnumVariant(_) => {}
        }

        Ok(implementations)
    }

    pub fn implementations_for_ty(
        &self,
        use_site: TargetRef,
        ty: &Ty,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let implementation_query = self.implementation_query(use_site);
        let mut implementations = Vec::new();
        for implementation in implementation_query.impls_for_ty(ty)? {
            Self::push_unique(&mut implementations, DeclarationRef::from(implementation));
        }
        Ok(implementations)
    }

    fn implementation_query(
        &self,
        use_site: TargetRef,
    ) -> ImplementationQuery<'_, &IndexedViewDb<'_>, &IndexedViewDb<'_>> {
        ImplementationQuery::new(
            ItemPathQuery::new(self.db, self.db),
            TargetItemQuery::new(self.db, self.db, use_site),
        )
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

    fn push_unique(declarations: &mut Vec<DeclarationRef>, declaration: DeclarationRef) {
        if !declarations.contains(&declaration) {
            declarations.push(declaration);
        }
    }
}
