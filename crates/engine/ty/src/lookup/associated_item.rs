//! Finds declarations reachable through the prefix of an associated-item path.
//!
//! A caller that has not selected the final path segment asks the permissive question: "what can
//! follow `Widget::`?"
//! A complete path such as `Widget::new` carries its name into the same lookup, selecting
//! candidate owners before proving impls and opening their member stores.
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

use std::{cmp::Ordering, collections::HashSet};

use rg_def_map::DefMapSource;
use rg_ir_model::{
    AssocItemId, ConstRef, EnumVariantRef, FunctionRef, ItemStoreRef, TraitApplicability,
    TraitDefRef, TypeAliasRef, TypeDefId,
};
use rg_semantic_ir::{AssociatedItemFilter, SemanticItemSource};
use rg_source::ReadError;

use crate::{
    AdtTy, Clause, Substitution, TraitApplication, Ty, TyContext,
    lookup::{ImplQuery, ItemPathQuery, ReceiverImplMatches},
    signature::SemanticSignatureQuery,
    solver::SolverScope,
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
/// A partially known receiver such as `Wrapper<_>` may only give the impl query enough
/// information for [`TraitApplicability::Maybe`]. Retaining the applicability lets callers keep a
/// permissive result without treating it as a proved trait match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssociatedItemCandidateRef {
    item: AssociatedItemRef,
    applicability: TraitApplicability,
    /// Whether the function's instantiated return type matched the expected type.
    /// `false` also covers unchecked functions and items such as associated constants.
    return_type_matches: bool,
}

impl AssociatedItemCandidateRef {
    pub fn new(
        item: AssociatedItemRef,
        applicability: TraitApplicability,
        return_type_matches: bool,
    ) -> Self {
        Self {
            item,
            applicability,
            return_type_matches,
        }
    }

    pub fn item(self) -> AssociatedItemRef {
        self.item
    }

    pub fn applicability(self) -> TraitApplicability {
        self.applicability
    }

    /// Whether lookup established a return type match using the qualifier's bindings.
    pub fn return_type_matches(self) -> bool {
        self.return_type_matches
    }

    /// Keep the more applicable result when two paths reach the same declaration.
    /// Equally applicable paths can instantiate the same function differently, as with
    /// `T: Source<u8> + Source<u16>`, so both must confirm the return type match.
    pub fn merge(self, other: Self) -> Self {
        debug_assert_eq!(self.item, other.item);
        match self.applicability.cmp(&other.applicability) {
            Ordering::Less => self,
            Ordering::Greater => other,
            Ordering::Equal => Self {
                return_type_matches: self.return_type_matches && other.return_type_matches,
                ..self
            },
        }
    }
}

type ReturnTypeMatcher<'a> = dyn Fn(FunctionRef, &Substitution) -> Result<bool, ReadError> + 'a;

/// Shared associated-item discovery after the prefix has been lowered to semantic types.
///
/// Body lookup supplies a resolver that understands local items. Signature lookup uses the
/// crate-level resolver. The candidate rules stay here so `Widget::`, `T::`, and
/// `<T as Factory>::` do not grow separate impl-selection behavior in each caller.
pub struct AssociatedItemQuery<'query, 'rank, D, I, R = ItemPathQuery<'query, D, I>> {
    context: TyContext<'query, D, I>,
    impls: ImplQuery<'query, D, I, R>,
    return_type_matcher: Option<&'rank ReturnTypeMatcher<'rank>>,
}

impl<'query, D, I> AssociatedItemQuery<'query, '_, D, I>
where
    D: DefMapSource + Clone,
    I: SemanticItemSource<'query> + Clone,
{
    pub fn new(context: TyContext<'query, D, I>) -> Self {
        let impls = ImplQuery::new(context.clone());
        Self {
            context,
            impls,
            return_type_matcher: None,
        }
    }
}

impl<'query, 'rank, D, I, R> AssociatedItemQuery<'query, 'rank, D, I, R>
where
    D: DefMapSource + Clone,
    I: SemanticItemSource<'query> + Clone,
    R: SolverScope,
{
    pub fn with_resolver(context: TyContext<'query, D, I>, resolver: R) -> Self {
        let impls = ImplQuery::with_resolver(context.clone(), resolver);
        Self {
            context,
            impls,
            return_type_matcher: None,
        }
    }

    /// Check function return types as lookup expands the items from each matched owner.
    /// The matcher receives the owner's bindings, such as `T = u8` from `Buffer::<u8>::new`.
    pub fn with_return_type_matcher(mut self, matcher: &'rank ReturnTypeMatcher<'rank>) -> Self {
        self.return_type_matcher = Some(matcher);
        self
    }

    /// Find declarations exposed by every applicable impl for one receiver type.
    ///
    /// Index routing and canonical header matching stay inside [`ImplQuery`]. A caller therefore
    /// asks the same question for `Widget`, `u32`, or `[Widget]`; enum variants and item-kind
    /// adaptation remain explicit here.
    pub fn candidates_for_ty(
        &self,
        receiver_ty: &Ty,
        filter: AssociatedItemFilter<'_>,
    ) -> Result<Vec<AssociatedItemCandidateRef>, ReadError> {
        let traits = self
            .context
            .item_lookup()
            .traits_with_associated_items(filter)?;
        let matches = self
            .impls
            .matches_for_receiver_with_traits(receiver_ty, traits, filter)?;
        self.candidates_for_matches(receiver_ty, &matches, filter)
    }

    /// Expand already-matched impls into stable associated-item declarations.
    ///
    /// Body lookup supplies a match set that includes current-body overlays. Keeping expansion
    /// separate lets it apply local shadowing without selecting the impl headers a second time.
    pub fn candidates_for_matches(
        &self,
        receiver_ty: &Ty,
        matches: &ReceiverImplMatches,
        filter: AssociatedItemFilter<'_>,
    ) -> Result<Vec<AssociatedItemCandidateRef>, ReadError> {
        let mut candidates = Vec::new();

        for nominal_ty in receiver_ty.as_adts() {
            if self.context.cancellation().is_cancelled() {
                return Ok(Default::default());
            }
            self.push_enum_variants(&mut candidates, nominal_ty, filter)?;
        }

        for impl_match in matches.inherent() {
            if self.context.cancellation().is_cancelled() {
                return Ok(Default::default());
            }
            self.push_assoc_items(
                &mut candidates,
                impl_match.impl_ref().member_store(),
                impl_match.applicability(),
                filter,
                Some(impl_match.subst()),
            )?;
        }

        let mut matching_traits = None;
        for selection in matches.traits() {
            if self.context.cancellation().is_cancelled() {
                return Ok(Default::default());
            }
            self.push_trait_hierarchy(
                &mut candidates,
                selection.application(),
                selection.applicability,
                filter,
                &mut matching_traits,
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
        filter: AssociatedItemFilter<'_>,
    ) -> Result<Vec<AssociatedItemCandidateRef>, ReadError> {
        let mut candidates = Vec::new();
        let mut matching_traits = None;
        for application in applications {
            if self.context.cancellation().is_cancelled() {
                return Ok(Default::default());
            }
            self.push_trait_hierarchy(
                &mut candidates,
                &application,
                applicability,
                filter,
                &mut matching_traits,
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
        filter: AssociatedItemFilter<'_>,
    ) -> Result<(), ReadError> {
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
        for (index, variant) in data.variants.iter().enumerate() {
            if self.context.cancellation().is_cancelled() {
                return Ok(());
            }
            if !filter.matches(variant.name.as_str()) {
                continue;
            }
            Self::push_candidate(
                candidates,
                AssociatedItemRef::EnumVariant(EnumVariantRef {
                    origin: receiver_ty.def.origin,
                    enum_id,
                    index,
                }),
                TraitApplicability::Yes,
                false,
            );
        }
        Ok(())
    }

    /// Add direct items and recursively inherited supertrait items.
    fn push_trait_hierarchy(
        &self,
        candidates: &mut Vec<AssociatedItemCandidateRef>,
        application: &TraitApplication,
        applicability: TraitApplicability,
        filter: AssociatedItemFilter<'_>,
        matching_traits: &mut Option<HashSet<TraitDefRef>>,
        lineage: &mut Vec<TraitDefRef>,
    ) -> Result<(), ReadError> {
        let trait_ref = application.def;
        if lineage.contains(&trait_ref) {
            return Ok(());
        }
        lineage.push(trait_ref);

        // `T: Child` can expose `Parent::Output` even when Child declares no Output itself.
        // Skip Child's member store when its name hints rule it out, but still read its header
        // below to reach Parent. Body-local declarations are not in the crate name index.
        let read_members = match filter {
            AssociatedItemFilter::Named(_) if trait_ref.origin.as_crate_ref().is_some() => {
                // Load the name index only when a crate trait needs it, then share membership
                // checks across all roots and supertraits instead of copying the list per trait.
                let traits = match matching_traits {
                    Some(traits) => traits,
                    None => matching_traits.insert(
                        self.context
                            .item_lookup()
                            .traits_with_associated_items(filter)?
                            .into_iter()
                            .collect(),
                    ),
                };
                traits.contains(&trait_ref)
            }
            _ => true,
        };
        let subst =
            if self.return_type_matcher.is_some() && applicability == TraitApplicability::Yes {
                let generics = self
                    .context
                    .item_paths()
                    .generics()
                    .generics(trait_ref.into())?;
                Some(Substitution::from_args(&generics, &application.args))
            } else {
                None
            };
        if read_members {
            self.push_assoc_items(
                candidates,
                trait_ref.member_store(),
                applicability,
                filter,
                subst.as_ref(),
            )?;
        }

        // The canonical trait header has already lowered `Self: Super` predicates. Filtering on
        // the trait's own `Self` excludes unrelated bounds on its other generic parameters.
        if let Some(header) =
            SemanticSignatureQuery::trait_header_from(self.context.item_paths(), trait_ref)?
        {
            for clause in &header.clauses {
                if self.context.cancellation().is_cancelled() {
                    return Ok(());
                }
                let Clause::Implemented(application) = clause else {
                    continue;
                };
                if application.def == trait_ref || application.self_ty() != Some(&header.self_ty) {
                    continue;
                }
                // Carry the child's arguments into the parent. For
                // `trait Child<T>: Parent<Vec<T>> {}`, a `Child<u8>` bound supplies
                // `Parent<Vec<u8>>` when looking up inherited methods.
                let instantiated;
                let application = if let Some(subst) = &subst {
                    instantiated = subst.apply_trait_application(application);
                    &instantiated
                } else {
                    application
                };
                self.push_trait_hierarchy(
                    candidates,
                    application,
                    applicability,
                    filter,
                    matching_traits,
                    lineage,
                )?;
            }
        }

        lineage.pop();
        Ok(())
    }

    fn push_assoc_items(
        &self,
        candidates: &mut Vec<AssociatedItemCandidateRef>,
        store: ItemStoreRef,
        applicability: TraitApplicability,
        filter: AssociatedItemFilter<'_>,
        owner_subst: Option<&Substitution>,
    ) -> Result<(), ReadError> {
        for item in self
            .context
            .item_paths()
            .items()
            .associated_items(store, filter)?
        {
            if self.context.cancellation().is_cancelled() {
                return Ok(());
            }
            let item = match item {
                AssocItemId::Function(id) => {
                    AssociatedItemRef::Function(FunctionRef { store, id: *id })
                }
                AssocItemId::TypeAlias(id) => {
                    AssociatedItemRef::TypeAlias(TypeAliasRef { store, id: *id })
                }
                AssocItemId::Const(id) => AssociatedItemRef::Const(ConstRef { store, id: *id }),
            };
            let return_type_matches = match (item, self.return_type_matcher, owner_subst) {
                (AssociatedItemRef::Function(function), Some(matcher), Some(subst))
                    if applicability == TraitApplicability::Yes =>
                {
                    matcher(function, subst)?
                }
                _ => false,
            };
            Self::push_candidate(candidates, item, applicability, return_type_matches);
        }
        Ok(())
    }

    /// Collapse overlapping impls by declaration while retaining the strongest evidence.
    fn push_candidate(
        candidates: &mut Vec<AssociatedItemCandidateRef>,
        item: AssociatedItemRef,
        applicability: TraitApplicability,
        return_type_matches: bool,
    ) {
        let candidate = AssociatedItemCandidateRef::new(item, applicability, return_type_matches);
        if let Some(existing) = candidates
            .iter_mut()
            .find(|candidate| candidate.item == item)
        {
            *existing = existing.merge(candidate);
            return;
        }
        candidates.push(candidate);
    }
}
