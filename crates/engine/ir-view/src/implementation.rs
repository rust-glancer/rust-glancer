//! Implementation lookup over indexed views.
//!
//! Definition navigation can select one impl from a call's facts; goto-implementation can collect
//! all explicit impl methods of a trait. Both need crate item indexes and body expression facts,
//! so this view keeps those storage-shaped queries out of analysis.

use anyhow::Context as _;
use rg_body_ir::BodyResolutionContext;
use rg_ir_model::{
    BodyRef, CrateRef, DefMapRef, FunctionRef, ItemOwner, SemanticItemRef, TypeDefRef,
    identity::DeclarationRef,
};
use rg_semantic_ir::ItemStoreQuery;
use rg_std::UniqueVec;
use rg_ty::{Ty, TyContext, lookup::ImplementationQuery};

use crate::{
    IndexedViewDb, body::BodyCallView, lookup::resolution::ResolutionView, ty::IndexedType,
};

/// Finds implementation declarations for types, traits, and methods.
pub struct ImplementationView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> ImplementationView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    /// Find a concrete impl method using the function and substitutions selected for this call.
    ///
    /// For `user.name()` with `user: User`, the facts can name `Named::name` even when
    /// `impl Named for User` provides the body. Return that impl's method if one impl is proven
    /// to apply. An inherited default body has no impl method to return; callers decide whether
    /// to use the trait declaration instead.
    pub fn selected_call_implementation(
        &self,
        call: &BodyCallView<'_>,
    ) -> anyhow::Result<Option<FunctionRef>> {
        let Some(facts) = call.facts() else {
            return Ok(None);
        };
        // Only a trait-owned function needs another lookup. An inherent method already names
        // its implementation, and a free function has no impl to look for.
        let items = ItemStoreQuery::new(self.db);
        let Some(function) = items
            .function_data(facts.function())
            .context("read selected function")?
        else {
            return Ok(None);
        };
        if !matches!(function.owner, ItemOwner::Trait(_)) {
            return Ok(None);
        }

        // An impl may be declared inside this body or require a bound from the enclosing
        // function. Give impl selection that context along with the saved crate declarations.
        let body_ref = call.expr.body_ir();
        let context = self
            .type_context(body_ref.crate_ref)
            .context("assemble call impl lookup")?;
        let scope = BodyResolutionContext::new(
            self.db,
            self.db,
            body_ref,
            call.body.structure(),
            context.item_lookup(),
            self.db.cancellation().clone(),
        );
        ImplementationQuery::new(context)
            .selected_call_implementation(facts.function(), facts.generic_args(), &scope)
            .context("select call implementation")
    }

    /// Return implementations for a declaration, retaining a completed empty answer.
    ///
    /// `Some(empty)` means that the declaration supports this query but has no targets. For
    /// example, a default-only trait method must not fall through to impls of its return type.
    /// `None` lets the caller ask a type-based question for other symbols, such as a field.
    pub fn implementations_for_declaration(
        &self,
        use_site: CrateRef,
        body_ref: Option<BodyRef>,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Option<UniqueVec<DeclarationRef>>> {
        let declaration = ResolutionView::new(self.db).canonical_declaration(declaration)?;
        let context = self.type_context(use_site)?;

        let mut implementations = UniqueVec::new();

        match declaration {
            DeclarationRef::Item(SemanticItemRef::TypeDef(ty)) => {
                if let DefMapRef::Body(body_ref) = ty.origin {
                    self.push_body_local_impls_for_type_def(&mut implementations, body_ref, ty)?;
                }
                for implementation in ImplementationQuery::new(context).impls_for_type_def(ty)? {
                    rg_std::check_cancel!(self.db, "implementation lookup");
                    implementations.push(DeclarationRef::from(implementation));
                }
            }
            DeclarationRef::Item(
                item @ (SemanticItemRef::Trait(_) | SemanticItemRef::Function(_)),
            ) => {
                // A call uses its surrounding body; a local declaration can also supply its
                // owning body. Only trait and function lookup need this scope to discover
                // local impls, including those in the owning modules of nested functions.
                let body_ref = body_ref.or_else(|| match declaration.origin() {
                    DefMapRef::Body(body) => Some(body),
                    DefMapRef::Crate(_) => None,
                });
                let body = body_ref
                    .map(|body| self.db.body_ir.body(body))
                    .transpose()?
                    .flatten();
                let scope = body_ref.zip(body).map(|(body_ref, body)| {
                    BodyResolutionContext::new(
                        self.db,
                        self.db,
                        body_ref,
                        body.structure(),
                        context.item_lookup(),
                        self.db.cancellation().clone(),
                    )
                });
                let implementation_query = ImplementationQuery::new(context);
                match item {
                    SemanticItemRef::Trait(trait_ref) => {
                        for implementation in
                            implementation_query.impls_for_trait(trait_ref, scope.as_ref())?
                        {
                            rg_std::check_cancel!(self.db, "implementation lookup");
                            implementations.push(DeclarationRef::from(implementation));
                        }
                    }
                    SemanticItemRef::Function(function) => {
                        for implementation in implementation_query
                            .function_implementations(function, scope.as_ref())?
                        {
                            implementations.push(DeclarationRef::from(implementation));
                        }
                    }
                    _ => unreachable!("body scope is only used for traits and functions"),
                }
            }
            DeclarationRef::BodyBinding(binding) => {
                let Some(body) = self.db.body_ir.body(binding.body)? else {
                    return Ok(Some(implementations));
                };
                let Some(binding_ty) = body.binding_ty(binding.binding) else {
                    return Ok(Some(implementations));
                };
                self.push_body_local_impls_for_ty(&mut implementations, binding.body, binding_ty)?;
                for implementation in ImplementationQuery::new(context).impls_for_ty(binding_ty)? {
                    rg_std::check_cancel!(self.db, "implementation lookup");
                    implementations.push(DeclarationRef::from(implementation));
                }
            }
            DeclarationRef::Module(_)
            | DeclarationRef::LocalDef(_)
            | DeclarationRef::Item(_)
            | DeclarationRef::Field(_)
            | DeclarationRef::EnumVariant(_) => return Ok(None),
        }

        Ok(Some(implementations))
    }

    /// Return impl blocks that apply to a type.
    pub fn implementations_for_ty(
        &self,
        use_site: CrateRef,
        ty: &IndexedType,
    ) -> anyhow::Result<UniqueVec<DeclarationRef>> {
        let mut implementations = UniqueVec::new();
        let implementation_query = ImplementationQuery::new(self.type_context(use_site)?);
        for implementation in implementation_query.impls_for_ty(ty.raw())? {
            rg_std::check_cancel!(self.db, "implementation lookup");
            implementations.push(DeclarationRef::from(implementation));
        }
        Ok(implementations)
    }

    /// Add impls declared in the same body item store as the selected local type.
    #[rg_std::cancelable("implementation lookup", token = self.db)]
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
            rg_std::check_cancel!(self.db, "implementation lookup");
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
        for candidate in ty.reference_chain() {
            rg_std::check_cancel!(self.db, "implementation lookup");
            for nominal in candidate.as_adts() {
                self.push_body_local_impls_for_type_def(implementations, body_ref, nominal.def)
                    .context("collect body-local nominal implementations")?;
            }
        }
        Ok(())
    }

    /// Build one crate lookup context for implementation discovery or selection.
    fn type_context(
        &self,
        use_site: CrateRef,
    ) -> anyhow::Result<TyContext<'_, &IndexedViewDb<'_>, &IndexedViewDb<'_>>> {
        let item_lookup_query = self.db.item_lookup_query(use_site)?;
        Ok(TyContext::new(
            self.db,
            self.db,
            item_lookup_query,
            use_site,
            self.db.cancellation().clone(),
        ))
    }
}
