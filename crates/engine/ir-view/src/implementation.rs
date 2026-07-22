//! Implementation lookup over indexed views.
//!
//! Goto-implementation is an editor query, but the lookup itself needs direct access to crate item
//! indexes and body expression facts. This view keeps those storage-shaped queries out of analysis.

use rg_body_ir::ExprKind;
use rg_ir_model::{
    BodyRef, CrateRef, DefMapRef, FunctionRef, SemanticItemRef, TraitDefRef, TypeDefRef,
    identity::{DeclarationRef, ExprRef},
};
use rg_semantic_ir::ItemStoreQuery;
use rg_std::UniqueVec;
use rg_ty::{ImplementationQuery, ReferencePeelingCandidates, Ty, TyContext};

use crate::{IndexedViewDb, lookup::resolution::ResolutionView, ty::IndexedType};

/// Finds implementation declarations for types, traits, and methods.
pub struct ImplementationView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> ImplementationView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    /// Return impl methods that may implement a resolved method call.
    pub fn method_call_implementations(
        &self,
        expr: ExprRef,
    ) -> anyhow::Result<Option<UniqueVec<DeclarationRef>>> {
        let body_ref = expr.body_ir();
        let Some(body_data) = self.db.body_ir.body(body_ref)? else {
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

        let Some(implementation_query) = self.implementation_query(body_ref.crate_ref)? else {
            return Ok(None);
        };
        let mut implementations = UniqueVec::new();
        for declaration in declarations {
            let Some(function) = self.function_ref_for_declaration(declaration)? else {
                continue;
            };
            for implementation in
                implementation_query.function_implementations(function, receiver_ty)?
            {
                implementations.push(DeclarationRef::from(implementation));
            }
        }
        Ok(Some(implementations))
    }

    /// Return implementations related to a declaration.
    pub fn implementations_for_declaration(
        &self,
        use_site: CrateRef,
        declaration: DeclarationRef,
    ) -> anyhow::Result<UniqueVec<DeclarationRef>> {
        let mut implementations = UniqueVec::new();
        let Some(implementation_query) = self.implementation_query(use_site)? else {
            return Ok(implementations);
        };

        match declaration {
            DeclarationRef::Item(item) => match item {
                SemanticItemRef::TypeDef(ty) => {
                    if let DefMapRef::Body(body_ref) = ty.origin {
                        self.push_body_local_impls_for_type_def(
                            &mut implementations,
                            body_ref,
                            ty,
                        )?;
                    }
                    for implementation in implementation_query.impls_for_type_def(ty)? {
                        implementations.push(DeclarationRef::from(implementation));
                    }
                }
                SemanticItemRef::Trait(trait_ref) => {
                    self.push_body_local_impls_for_trait(&mut implementations, trait_ref)?;
                    for implementation in implementation_query.impls_for_trait(trait_ref)? {
                        implementations.push(DeclarationRef::from(implementation));
                    }
                }
                SemanticItemRef::Function(function) => {
                    for implementation in
                        implementation_query.function_implementations(function, None)?
                    {
                        implementations.push(DeclarationRef::from(implementation));
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
                    implementations.push(DeclarationRef::from(implementation));
                }
            }
            DeclarationRef::BodyBinding(binding) => {
                let Some(body) = self.db.body_ir.body(binding.body)? else {
                    return Ok(implementations);
                };
                let Some(binding_ty) = body.binding_ty(binding.binding) else {
                    return Ok(implementations);
                };
                self.push_body_local_impls_for_ty(&mut implementations, binding.body, binding_ty)?;
                for implementation in implementation_query.impls_for_ty(binding_ty)? {
                    implementations.push(DeclarationRef::from(implementation));
                }
            }
            DeclarationRef::Module(_)
            | DeclarationRef::Field(_)
            | DeclarationRef::EnumVariant(_) => {}
        }

        Ok(implementations)
    }

    /// Return impl blocks that apply to a type.
    pub fn implementations_for_ty(
        &self,
        use_site: CrateRef,
        ty: &IndexedType,
    ) -> anyhow::Result<UniqueVec<DeclarationRef>> {
        let mut implementations = UniqueVec::new();
        let Some(implementation_query) = self.implementation_query(use_site)? else {
            return Ok(implementations);
        };
        for implementation in implementation_query.impls_for_ty(ty.raw())? {
            implementations.push(DeclarationRef::from(implementation));
        }
        Ok(implementations)
    }

    /// Add impls declared in the same body item store as the selected local type.
    fn push_body_local_impls_for_type_def(
        &self,
        implementations: &mut UniqueVec<DeclarationRef>,
        body_ref: BodyRef,
        ty: TypeDefRef,
    ) -> anyhow::Result<()> {
        let Some(store) = self.db.body_ir.body_item_store(body_ref)? else {
            return Ok(());
        };

        for (impl_ref, impl_data) in store.impls_with_refs() {
            if impl_data.resolved_self_ty.is(&ty) {
                implementations.push(DeclarationRef::from(impl_ref));
            }
        }
        Ok(())
    }

    /// Add impls from the body that owns a binding whose type is being inspected.
    fn push_body_local_impls_for_ty(
        &self,
        implementations: &mut UniqueVec<DeclarationRef>,
        body_ref: BodyRef,
        ty: &Ty,
    ) -> anyhow::Result<()> {
        for candidate in ReferencePeelingCandidates::new(ty) {
            for nominal in candidate.ty().as_adts() {
                self.push_body_local_impls_for_type_def(implementations, body_ref, nominal.def)?;
            }
        }
        Ok(())
    }

    /// Add impls declared next to a body-local trait.
    fn push_body_local_impls_for_trait(
        &self,
        implementations: &mut UniqueVec<DeclarationRef>,
        trait_ref: TraitDefRef,
    ) -> anyhow::Result<()> {
        let DefMapRef::Body(body_ref) = trait_ref.origin else {
            return Ok(());
        };
        let Some(store) = self.db.body_ir.body_item_store(body_ref)? else {
            return Ok(());
        };

        for (impl_ref, impl_data) in store.impls_with_refs() {
            if impl_data.resolved_trait_ref.is(&trait_ref) {
                implementations.push(DeclarationRef::from(impl_ref));
            }
        }
        Ok(())
    }

    /// Build the lower implementation query for one crate.
    fn implementation_query(
        &self,
        use_site: CrateRef,
    ) -> anyhow::Result<Option<ImplementationQuery<'_, &IndexedViewDb<'_>, &IndexedViewDb<'_>>>>
    {
        let Some(item_lookup_index) = self.db.body_ir.item_lookup_index(use_site)? else {
            return Ok(None);
        };
        Ok(Some(ImplementationQuery::new(TyContext::new(
            self.db,
            self.db,
            item_lookup_index,
            self.db.trait_selection(use_site),
        ))))
    }

    /// Extract a function ref from a declaration when it denotes a function.
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
}
