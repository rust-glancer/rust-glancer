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
    AssocItemId, ConstRef, EnumVariantRef, FunctionRef, TraitApplicability, TraitDefRef,
    TypeAliasRef, TypeDefId,
};
use rg_semantic_ir::ItemStoreSource;

use crate::{
    AdtTy, Clause, ImplMatcher, ItemPathQuery, ReceiverImplMatches, TraitApplication, Ty,
    TyContext, TypePathResolver, inference::InferenceTable,
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

    /// Find declarations exposed by every applicable impl for one receiver type.
    ///
    /// Index routing and canonical header matching stay inside [`ImplMatcher`]. A caller therefore
    /// asks the same question for `Widget`, `u32`, or `[Widget]`; enum variants and item-kind
    /// adaptation remain explicit here.
    pub fn candidates_for_ty(
        &self,
        receiver_ty: &Ty,
    ) -> Result<Vec<AssociatedItemCandidateRef>, D::Error> {
        let table = InferenceTable::new();
        let matches = self.matcher.matches_for_receiver_with_traits(
            receiver_ty,
            self.context.item_lookup().traits_with_associated_items(),
            &table,
        )?;
        self.candidates_for_matches(receiver_ty, &matches)
    }

    /// Expand already-matched impls into stable associated-item declarations.
    ///
    /// Body lookup supplies a match set that includes current-body overlays. Keeping expansion
    /// separate lets it apply local shadowing without selecting the impl headers a second time.
    pub fn candidates_for_matches(
        &self,
        receiver_ty: &Ty,
        matches: &ReceiverImplMatches,
    ) -> Result<Vec<AssociatedItemCandidateRef>, D::Error> {
        let mut candidates = Vec::new();

        for nominal_ty in receiver_ty.as_adts() {
            self.push_enum_variants(&mut candidates, nominal_ty)?;
        }

        for impl_match in matches.inherent() {
            let Some(data) = self
                .context
                .item_paths()
                .items()
                .impl_data(impl_match.impl_ref())?
            else {
                continue;
            };
            self.push_assoc_items(
                &mut candidates,
                impl_match.impl_ref().origin,
                &data.items,
                impl_match.applicability(),
            );
        }

        for selection in matches.traits() {
            self.push_trait_hierarchy(
                &mut candidates,
                selection.trait_impl.trait_ref,
                selection.applicability,
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
