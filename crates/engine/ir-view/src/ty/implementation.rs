//! Composite implementation lookup over semantic and body-local declarations.
//!
//! Goto-implementation asks for source declarations that implement a selected type, trait, or
//! trait method. This view owns the storage-specific lookup rules so query code can stay focused on
//! cursor policy and editor projection.

use rg_body_ir::ExprKind;
use rg_ir_model::{
    AssocItemId, FunctionRef, ImplRef, ItemOwner, SemanticItemRef, TraitRef, TypeDefRef,
    identity::{DeclarationRef, ExprRef},
};
use rg_ir_storage::ItemStoreQuery;
use rg_ty::{Autoderef, AutoderefMode, ImplMatcher, ItemPathQuery, ReferencePeelingCandidates, Ty};

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
        for declaration in declarations {
            self.extend_function_implementations(&mut implementations, declaration, receiver_ty)?;
        }
        Ok(Some(implementations))
    }

    pub fn implementations_for_declaration(
        &self,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let mut implementations = Vec::new();

        match declaration {
            DeclarationRef::Item(item) => match item {
                SemanticItemRef::TypeDef(ty) => {
                    self.extend_type_def_implementations(&mut implementations, ty)?;
                }
                SemanticItemRef::Trait(trait_ref) => {
                    self.extend_trait_implementations(&mut implementations, trait_ref)?;
                }
                SemanticItemRef::Function(function) => {
                    self.extend_function_implementations(
                        &mut implementations,
                        DeclarationRef::from(function),
                        None,
                    )?;
                }
                SemanticItemRef::Impl(_)
                | SemanticItemRef::TypeAlias(_)
                | SemanticItemRef::Const(_)
                | SemanticItemRef::Static(_) => {}
            },
            DeclarationRef::LocalDef(local_def) => {
                let Some(SemanticItemRef::Function(function)) =
                    ItemStoreQuery::new(self.db).semantic_item_for_local_def(local_def)?
                else {
                    return Ok(implementations);
                };
                self.extend_function_implementations(
                    &mut implementations,
                    DeclarationRef::from(function),
                    None,
                )?;
            }
            DeclarationRef::BodyBinding(binding) => {
                let Some(body) = self.db.body_ir.body_data(binding.body)? else {
                    return Ok(implementations);
                };
                let Some(binding_data) = body.binding(binding.binding) else {
                    return Ok(implementations);
                };
                self.extend_ty_implementations(&mut implementations, &binding_data.ty)?;
            }
            DeclarationRef::Module(_)
            | DeclarationRef::Field(_)
            | DeclarationRef::EnumVariant(_) => {}
        }

        Ok(implementations)
    }

    pub fn implementations_for_ty(&self, ty: &Ty) -> anyhow::Result<Vec<DeclarationRef>> {
        let mut implementations = Vec::new();
        self.extend_ty_implementations(&mut implementations, ty)?;
        Ok(implementations)
    }

    fn extend_ty_implementations(
        &self,
        implementations: &mut Vec<DeclarationRef>,
        ty: &Ty,
    ) -> anyhow::Result<()> {
        for candidate in ReferencePeelingCandidates::new(ty) {
            for ty in candidate.ty().as_nominals() {
                self.extend_type_def_implementations(implementations, ty.def)?;
            }
        }
        Ok(())
    }

    fn extend_type_def_implementations(
        &self,
        implementations: &mut Vec<DeclarationRef>,
        ty: TypeDefRef,
    ) -> anyhow::Result<()> {
        for impl_ref in ItemStoreQuery::new(self.db).impls_for_type(ty)? {
            Self::push_unique(implementations, DeclarationRef::from(impl_ref));
        }
        Ok(())
    }

    fn extend_trait_implementations(
        &self,
        implementations: &mut Vec<DeclarationRef>,
        trait_ref: TraitRef,
    ) -> anyhow::Result<()> {
        for impl_ref in ItemStoreQuery::new(self.db).impls_for_trait(trait_ref)? {
            Self::push_unique(implementations, DeclarationRef::from(impl_ref));
        }
        Ok(())
    }

    fn extend_function_implementations(
        &self,
        implementations: &mut Vec<DeclarationRef>,
        declaration: DeclarationRef,
        receiver_ty: Option<&Ty>,
    ) -> anyhow::Result<()> {
        match declaration {
            DeclarationRef::Module(_) => Ok(()),
            DeclarationRef::LocalDef(local_def) => {
                let Some(SemanticItemRef::Function(function)) =
                    ItemStoreQuery::new(self.db).semantic_item_for_local_def(local_def)?
                else {
                    return Ok(());
                };
                self.extend_semantic_function_implementations(
                    implementations,
                    function,
                    receiver_ty,
                )
            }
            DeclarationRef::Item(SemanticItemRef::Function(function)) => self
                .extend_semantic_function_implementations(implementations, function, receiver_ty),
            DeclarationRef::Item(_)
            | DeclarationRef::Field(_)
            | DeclarationRef::EnumVariant(_)
            | DeclarationRef::BodyBinding(_) => Ok(()),
        }
    }

    fn extend_semantic_function_implementations(
        &self,
        implementations: &mut Vec<DeclarationRef>,
        function: FunctionRef,
        receiver_ty: Option<&Ty>,
    ) -> anyhow::Result<()> {
        let Some(data) = ItemStoreQuery::new(self.db).function_data(function)? else {
            return Ok(());
        };

        match data.owner {
            ItemOwner::Trait(trait_id) => {
                let trait_ref = TraitRef {
                    origin: function.origin,
                    id: trait_id,
                };
                match receiver_ty {
                    Some(receiver_ty) => self.extend_trait_method_implementations_for_receiver(
                        implementations,
                        trait_ref,
                        data.name.as_str(),
                        receiver_ty,
                    ),
                    None => self.extend_trait_method_implementations(
                        implementations,
                        trait_ref,
                        data.name.as_str(),
                    ),
                }
            }
            ItemOwner::Impl(_) => {
                Self::push_unique(implementations, DeclarationRef::from(function));
                Ok(())
            }
            ItemOwner::Module(_) => Ok(()),
        }
    }

    fn extend_trait_method_implementations_for_receiver(
        &self,
        implementations: &mut Vec<DeclarationRef>,
        trait_ref: TraitRef,
        method_name: &str,
        receiver_ty: &Ty,
    ) -> anyhow::Result<()> {
        let autoderef = Autoderef::new(ItemPathQuery::new(self.db, self.db));
        let matcher = ImplMatcher::new(ItemPathQuery::new(self.db, self.db));
        for candidate in autoderef.candidates(AutoderefMode::MethodReceiver, receiver_ty) {
            let candidate = candidate?;
            for ty in candidate.ty().as_nominals() {
                for trait_impl in ItemStoreQuery::new(self.db).trait_impls_for_type(ty.def)? {
                    if trait_impl.trait_ref != trait_ref {
                        continue;
                    }
                    // The nominal type match can still include generic impls for other concrete
                    // args. Reuse method lookup's applicability check so goto-implementation
                    // follows the receiver the user actually called the method on.
                    if !matcher
                        .trait_impl_applicability(trait_impl, ty)?
                        .is_applicable()
                    {
                        continue;
                    }
                    self.extend_matching_impl_methods(
                        implementations,
                        trait_impl.impl_ref,
                        method_name,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn extend_trait_method_implementations(
        &self,
        implementations: &mut Vec<DeclarationRef>,
        trait_ref: TraitRef,
        method_name: &str,
    ) -> anyhow::Result<()> {
        for impl_ref in ItemStoreQuery::new(self.db).impls_for_trait(trait_ref)? {
            self.extend_matching_impl_methods(implementations, impl_ref, method_name)?;
        }
        Ok(())
    }

    fn extend_matching_impl_methods(
        &self,
        implementations: &mut Vec<DeclarationRef>,
        impl_ref: ImplRef,
        method_name: &str,
    ) -> anyhow::Result<()> {
        let Some(data) = ItemStoreQuery::new(self.db).impl_data(impl_ref)? else {
            return Ok(());
        };

        for item in &data.items {
            let &AssocItemId::Function(id) = item else {
                continue;
            };
            let function = FunctionRef {
                origin: impl_ref.origin,
                id,
            };
            let Some(function_data) = ItemStoreQuery::new(self.db).function_data(function)? else {
                continue;
            };
            if function_data.name.as_str() != method_name {
                continue;
            }
            Self::push_unique(implementations, DeclarationRef::from(function));
        }
        Ok(())
    }

    fn push_unique(implementations: &mut Vec<DeclarationRef>, declaration: DeclarationRef) {
        if !implementations.contains(&declaration) {
            implementations.push(declaration);
        }
    }
}
