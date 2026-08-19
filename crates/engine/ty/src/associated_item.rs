//! Finds declarations reachable through the prefix of an associated-item path.
//!
//! A caller that has not selected the final path segment asks the permissive question: "what can
//! follow `Widget::`?"
//!
//! ```text
//! trait Parent {
//!     const PARENT: usize;
//! }
//!
//! trait Factory: Parent {
//!     type Output;
//!     fn make() -> Self::Output;
//! }
//!
//! struct Widget;
//!
//! impl Widget {
//!     fn new() -> Self { Widget }
//! }
//!
//! impl Factory for Widget { /* ... */ }
//!
//! Widget::/* new, Output, make, PARENT */
//! ```
//!
//! A nominal receiver such as `Widget` contributes inherent items and items from matching trait
//! impls. A generic receiver such as `T: Factory` contributes items from the written trait bound.
//! In both cases supertraits are followed, which is why `PARENT` appears in the example.
//!
//! This module returns stable declaration identities and match confidence. It deliberately does
//! not decide labels, visibility at the cursor, or insertion text; those remain with the body and
//! editor-facing adapters.

use rg_def_map::DefMapSource;
use rg_ir_model::{
    AssocItemId, ConstRef, EnumVariantRef, FunctionRef, ImplRef, TraitApplicability, TraitDefRef,
    TraitImplRef, TypeAliasRef, TypeDefId,
};
use rg_semantic_ir::ItemStoreSource;
use rg_std::UniqueVec;

use crate::{
    AdtTy, Clause, ImplMatcher, ItemPathQuery, TraitApplication, Ty, TyContext, TypePathResolver,
    inference::InferenceTable,
};

/// Stable declaration identity returned by associated-item discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssociatedItemRef {
    Function(FunctionRef),
    TypeAlias(TypeAliasRef),
    Const(ConstRef),
    EnumVariant(EnumVariantRef),
}

/// One declaration together with how confidently its impl matched the receiver.
///
/// A partially known receiver such as `Wrapper<_>` may only give the impl matcher enough
/// information for [`TraitApplicability::Maybe`]. Retaining the applicability lets callers keep a
/// permissive result without treating it as a proved trait match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssociatedItemCandidateRef {
    item: AssociatedItemRef,
    applicability: TraitApplicability,
}

impl AssociatedItemCandidateRef {
    pub fn item(self) -> AssociatedItemRef {
        self.item
    }

    pub fn applicability(self) -> TraitApplicability {
        self.applicability
    }
}

/// Shared associated-item discovery after the prefix has been lowered to semantic types.
///
/// Body lookup supplies a resolver that understands local items. Signature lookup uses the
/// crate-level resolver. The candidate rules stay here so `Widget::`, `T::`, and
/// `<T as Factory>::` do not grow separate impl-selection behavior in each caller.
pub struct AssociatedItemQuery<'query, D, I, R = ItemPathQuery<'query, D, I>> {
    context: TyContext<'query, D, I>,
    matcher: ImplMatcher<'query, D, I, R>,
}

impl<'query, D, I> AssociatedItemQuery<'query, D, I>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
{
    pub fn new(context: TyContext<'query, D, I>) -> Self {
        let matcher = ImplMatcher::new(context.clone());
        Self { context, matcher }
    }
}

impl<'query, D, I, R> AssociatedItemQuery<'query, D, I, R>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    pub fn with_resolver(context: TyContext<'query, D, I>, resolver: R) -> Self {
        let matcher = ImplMatcher::with_resolver(context.clone(), resolver);
        Self { context, matcher }
    }

    /// Find everything available through a concrete struct, enum, or union.
    ///
    /// For an enum, this combines three sources that share the same `Type::name` syntax:
    ///
    /// ```text
    /// enum State { Ready }
    ///
    /// impl State {
    ///     fn parse() -> State { State::Ready }
    /// }
    ///
    /// trait Reset {
    ///     fn reset() -> State;
    /// }
    ///
    /// impl Reset for State { /* ... */ }
    ///
    /// State::/* Ready, parse, reset */
    /// ```
    ///
    /// Trait-provided candidates come from the trait declaration, not the impl body. That keeps
    /// default items and the trait's documentation available even when a concrete impl overrides
    /// the item.
    pub fn candidates_for_nominal(
        &self,
        receiver_ty: &AdtTy,
    ) -> Result<Vec<AssociatedItemCandidateRef>, D::Error> {
        self.candidates_for_nominal_from_impls(
            receiver_ty,
            self.context
                .lookup_index()
                .inherent_impls_for_type(receiver_ty.def),
            self.context
                .lookup_index()
                .trait_impls_for_type(receiver_ty.def)
                .cloned()
                .unwrap_or_default(),
            true,
        )
    }

    /// Return candidates from an explicitly selected impl universe.
    ///
    /// Body IR uses this entry point for its local-item overlay, while crate-level callers use
    /// [`Self::candidates_for_nominal`]. `include_variants` lets the overlay avoid adding the same
    /// enum constructors a second time.
    pub fn candidates_for_nominal_from_impls(
        &self,
        receiver_ty: &AdtTy,
        inherent_impls: UniqueVec<ImplRef>,
        trait_impls: UniqueVec<TraitImplRef>,
        include_variants: bool,
    ) -> Result<Vec<AssociatedItemCandidateRef>, D::Error> {
        let mut candidates = Vec::new();

        if include_variants {
            self.push_enum_variants(&mut candidates, receiver_ty)?;
        }

        // Inherent items belong to the selected impl itself. Preserve a tentative structural
        // match as `Maybe` when generic information is incomplete, but do not let downstream
        // consumers mistake it for a proved match.
        for impl_ref in inherent_impls {
            let Some(data) = self.context.item_paths().items().impl_data(impl_ref)? else {
                continue;
            };
            if !data.resolved_self_ty.is(&receiver_ty.def) {
                continue;
            }
            let Some((_, applicability)) = self
                .matcher
                .impl_self_subst_for_impl(impl_ref, &Ty::adt(receiver_ty.clone()))?
            else {
                continue;
            };
            if !applicability.is_applicable() {
                continue;
            }
            self.push_assoc_items(&mut candidates, impl_ref.origin, &data.items, applicability);
        }

        // Trait items are presented from the trait declaration rather than an implementation.
        // This retains defaulted items and stable docs while impl selection supplies confidence.
        let table = InferenceTable::new();
        for trait_impl in trait_impls {
            let applicability =
                self.matcher
                    .trait_impl_applicability(trait_impl, receiver_ty, &table)?;
            if !applicability.is_applicable() {
                continue;
            }
            self.push_trait_hierarchy(
                &mut candidates,
                trait_impl.trait_ref,
                applicability,
                &mut Vec::new(),
            )?;
        }

        Ok(candidates)
    }

    /// Find items exposed by written trait bounds, including their supertraits.
    ///
    /// This is the path used for a generic prefix:
    ///
    /// ```text
    /// trait Parent {
    ///     type ParentItem;
    /// }
    ///
    /// trait Factory: Parent {
    ///     type Output;
    ///     fn make() -> Self::Output;
    /// }
    ///
    /// fn build<T: Factory>() {
    ///     T::/* Output, make, ParentItem */
    /// }
    /// ```
    ///
    /// The caller has already decided which trait applications constrain `T`; this method expands
    /// those traits into declaration candidates and walks the supertrait chain.
    pub fn candidates_for_trait_applications(
        &self,
        applications: impl IntoIterator<Item = TraitApplication>,
        applicability: TraitApplicability,
    ) -> Result<Vec<AssociatedItemCandidateRef>, D::Error> {
        let mut candidates = Vec::new();
        for application in applications {
            self.push_trait_hierarchy(
                &mut candidates,
                application.def,
                applicability,
                &mut Vec::new(),
            )?;
        }
        Ok(candidates)
    }

    /// Add variants from an enum receiver before impl-owned associated items.
    fn push_enum_variants(
        &self,
        candidates: &mut Vec<AssociatedItemCandidateRef>,
        receiver_ty: &AdtTy,
    ) -> Result<(), D::Error> {
        let TypeDefId::Enum(enum_id) = receiver_ty.def.id else {
            return Ok(());
        };
        let Some(data) = self
            .context
            .item_paths()
            .items()
            .enum_data_for_type_def(receiver_ty.def)?
        else {
            return Ok(());
        };
        for index in 0..data.variants.len() {
            Self::push_candidate(
                candidates,
                AssociatedItemRef::EnumVariant(EnumVariantRef {
                    origin: receiver_ty.def.origin,
                    enum_id,
                    index,
                }),
                TraitApplicability::Yes,
            );
        }
        Ok(())
    }

    /// Add direct items and recursively inherited supertrait items.
    fn push_trait_hierarchy(
        &self,
        candidates: &mut Vec<AssociatedItemCandidateRef>,
        trait_ref: TraitDefRef,
        applicability: TraitApplicability,
        lineage: &mut Vec<TraitDefRef>,
    ) -> Result<(), D::Error> {
        if lineage.contains(&trait_ref) {
            return Ok(());
        }
        lineage.push(trait_ref);

        let Some(data) = self.context.item_paths().items().trait_data(trait_ref)? else {
            lineage.pop();
            return Ok(());
        };
        self.push_assoc_items(candidates, trait_ref.origin, &data.items, applicability);

        // The canonical trait header has already lowered `Self: Super` predicates. Filtering on
        // the trait's own `Self` excludes unrelated bounds on its other generic parameters.
        if let Some(header) = self
            .context
            .trait_selection()
            .trait_header_with(self.context.item_paths(), trait_ref)?
        {
            for clause in &header.clauses {
                let Clause::Implemented(application) = clause else {
                    continue;
                };
                if application.def == trait_ref || application.self_ty() != Some(&header.self_ty) {
                    continue;
                }
                self.push_trait_hierarchy(candidates, application.def, applicability, lineage)?;
            }
        }

        lineage.pop();
        Ok(())
    }

    fn push_assoc_items(
        &self,
        candidates: &mut Vec<AssociatedItemCandidateRef>,
        origin: rg_ir_model::DefMapRef,
        items: &[AssocItemId],
        applicability: TraitApplicability,
    ) {
        for item in items {
            let item = match item {
                AssocItemId::Function(id) => {
                    AssociatedItemRef::Function(FunctionRef { origin, id: *id })
                }
                AssocItemId::TypeAlias(id) => {
                    AssociatedItemRef::TypeAlias(TypeAliasRef { origin, id: *id })
                }
                AssocItemId::Const(id) => AssociatedItemRef::Const(ConstRef { origin, id: *id }),
            };
            Self::push_candidate(candidates, item, applicability);
        }
    }

    /// Collapse overlapping impls by declaration while retaining the strongest evidence.
    fn push_candidate(
        candidates: &mut Vec<AssociatedItemCandidateRef>,
        item: AssociatedItemRef,
        applicability: TraitApplicability,
    ) {
        if let Some(existing) = candidates
            .iter_mut()
            .find(|candidate| candidate.item == item)
        {
            existing.applicability = existing.applicability.or(applicability);
            return;
        }
        candidates.push(AssociatedItemCandidateRef {
            item,
            applicability,
        });
    }
}
