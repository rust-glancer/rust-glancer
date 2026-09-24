//! Receiver matching after the relevant impl universe has been discovered.
//!
//! Inherent items start from the receiver, because an inherent impl belongs to that receiver shape.
//! Trait items start from a declaration name or completion surface, which identifies relevant
//! traits before this module narrows each trait's impls by `Self`. Both paths finish here and retain
//! the evidence needed to instantiate an item selected from the matching impl.

use rg_def_map::DefMapSource;
use rg_ir_model::{DefMapRef, FunctionRef, ImplRef, TraitApplicability, TraitDefRef, TraitImplRef};
use rg_semantic_ir::ItemStoreSource;
use rg_std::UniqueVec;

use super::ImplQuery;
use crate::{Substitution, Ty, solver::SolverScope, trait_selection::TraitSelection};

/// One inherent impl whose canonical `Self` header matched a receiver.
///
/// For `impl<T> [T]` and receiver `[User]`, `subst` retains `T = User`. Item adapters must carry
/// this match forward instead of recovering the same binding from the impl header a second time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InherentImplMatch {
    impl_ref: ImplRef,
    subst: Substitution,
    applicability: TraitApplicability,
}

impl InherentImplMatch {
    pub fn impl_ref(&self) -> ImplRef {
        self.impl_ref
    }

    pub fn subst(&self) -> &Substitution {
        &self.subst
    }

    pub fn applicability(&self) -> TraitApplicability {
        self.applicability
    }
}

/// Applicable inherent and trait impls for one canonical receiver type.
///
/// Inherent matches retain owner substitutions. Trait matches retain a [`TraitSelection`]
/// because it also carries the instantiated trait arguments and how much was proved. Both are
/// owned results after the trial table has been released. Consumers can adapt them into methods,
/// associated functions, constants, or completion declarations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReceiverImplMatches {
    inherent: UniqueVec<InherentImplMatch>,
    traits: UniqueVec<TraitSelection>,
}

impl ReceiverImplMatches {
    pub fn inherent(&self) -> &[InherentImplMatch] {
        self.inherent.as_slice()
    }

    pub fn traits(&self) -> &[TraitSelection] {
        self.traits.as_slice()
    }

    /// Append another visible impl universe while preserving discovery order.
    ///
    /// Exact duplicate matches collapse here. Overlapping trait impls with different selections
    /// remain separate candidates because their proof evidence makes the values unequal.
    pub fn extend(&mut self, other: Self) {
        self.inherent.extend(other.inherent);
        self.traits.extend(other.traits);
    }
}

/// One function declaration together with the impl evidence that exposed it.
#[derive(Debug, PartialEq, Eq)]
pub struct ReceiverFunctionCandidate {
    function: FunctionRef,
    source: ReceiverFunctionSource,
}

impl ReceiverFunctionCandidate {
    pub fn function(&self) -> FunctionRef {
        self.function
    }

    pub fn inherent_match(&self) -> Option<&InherentImplMatch> {
        match &self.source {
            ReceiverFunctionSource::Inherent { impl_match } => Some(impl_match),
            ReceiverFunctionSource::Trait { .. } => None,
        }
    }

    pub fn into_trait_selection(self) -> Option<TraitSelection> {
        match self.source {
            ReceiverFunctionSource::Trait { selection } => Some(selection),
            ReceiverFunctionSource::Inherent { .. } => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ReceiverFunctionSource {
    Inherent { impl_match: InherentImplMatch },
    Trait { selection: TraitSelection },
}

impl<'query, D, I, R> ImplQuery<'query, D, I, R>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
    R: SolverScope<Error = D::Error>,
{
    /// Match inherent impls plus an already-discovered set of relevant traits.
    pub fn matches_for_receiver_with_traits(
        &self,
        receiver_ty: &Ty,
        trait_refs: impl IntoIterator<Item = TraitDefRef>,
    ) -> Result<ReceiverImplMatches, D::Error> {
        let mut matches = self.inherent_matches_for_receiver(receiver_ty)?;
        matches.extend(self.trait_matches_for_receiver(receiver_ty, trait_refs)?);
        Ok(matches)
    }

    /// Match only impls of already-discovered traits for one receiver.
    ///
    /// Named method calls use this after the inherent lane produced no applicable method. Broad
    /// completion still uses [`Self::matches_for_receiver_with_traits`] so it can expose both
    /// declaration families at once.
    pub fn trait_matches_for_receiver(
        &self,
        receiver_ty: &Ty,
        trait_refs: impl IntoIterator<Item = TraitDefRef>,
    ) -> Result<ReceiverImplMatches, D::Error> {
        let mut matches = ReceiverImplMatches::default();
        matches
            .traits
            .extend(self.trait_selections_for_receiver(receiver_ty, trait_refs)?);
        Ok(matches)
    }

    /// Match saved inherent impls for one receiver without opening any trait candidate universe.
    fn inherent_matches_for_receiver(
        &self,
        receiver_ty: &Ty,
    ) -> Result<ReceiverImplMatches, D::Error> {
        // Matching uses the solver's fail-soft exit when discovery is cancelled. Its operation
        // owner rejects the result through the shared token before publishing body facts or UI.
        let mut inherent_impls = UniqueVec::new();
        for receiver in receiver_ty.as_adts() {
            if self.context.cancellation().is_cancelled() {
                return Ok(Default::default());
            }
            let Ok(candidates) = self
                .context
                .item_lookup()
                .inherent_impls_for_type(receiver.def)
            else {
                return Ok(ReceiverImplMatches::default());
            };
            inherent_impls.extend(candidates);
        }

        let mut matches =
            self.inherent_matches_for_receiver_from_impls(receiver_ty, inherent_impls)?;

        // Concrete builtin-shaped receivers have no `TypeDefRef` index key. Keep this routing rule
        // beside the structural index it selects: being "unkeyed" is a property of the lookup
        // strategy, not a general property that the type model needs to expose.
        let has_structural_self_head = matches!(
            receiver_ty,
            Ty::Unit
                | Ty::Never
                | Ty::Primitive(_)
                | Ty::Tuple(_)
                | Ty::Array { .. }
                | Ty::Slice(_)
                | Ty::Reference { .. }
                | Ty::RawPointer { .. }
                | Ty::FnPointer { .. }
                | Ty::Closure(_)
                | Ty::FnDef(_)
        );
        if has_structural_self_head {
            let Ok(candidates) = self.context.item_lookup().structural_inherent_impls() else {
                return Ok(matches);
            };
            matches.extend(self.inherent_matches_for_receiver_from_impls(receiver_ty, candidates)?);
        }

        Ok(matches)
    }

    /// Select saved impls for traits already discovered from an item name or completion surface.
    fn trait_selections_for_receiver(
        &self,
        receiver_ty: &Ty,
        trait_refs: impl IntoIterator<Item = TraitDefRef>,
    ) -> Result<UniqueVec<TraitSelection>, D::Error> {
        let mut selections = UniqueVec::new();

        for trait_ref in trait_refs {
            if self.context.cancellation().is_cancelled() {
                return Ok(Default::default());
            }
            let Some(candidates) =
                super::trait_impl_candidates(&self.context, trait_ref, receiver_ty)
            else {
                // Cancellation leaves discovery incomplete; the request owner rejects it.
                break;
            };
            selections
                .extend(self.trait_selections_for_receiver_from_impls(receiver_ty, candidates)?);
        }

        Ok(selections)
    }

    /// Select an explicit, already-narrowed impl set, such as a current-body overlay.
    fn trait_selections_for_receiver_from_impls(
        &self,
        receiver_ty: &Ty,
        trait_impls: impl IntoIterator<Item = TraitImplRef>,
    ) -> Result<UniqueVec<TraitSelection>, D::Error> {
        let mut selections = UniqueVec::new();
        for trait_impl in trait_impls {
            if self.context.cancellation().is_cancelled() {
                return Ok(Default::default());
            }
            let Some(selection) = self.trait_impl_selection_for_ty(trait_impl, receiver_ty)? else {
                continue;
            };
            selections.push(selection);
        }
        Ok(selections)
    }

    /// Match a caller-selected impl universe while retaining all instantiation evidence.
    ///
    /// Body lookup uses this for current-body overlays; explicitly qualified trait paths use it
    /// for the already narrowed impl set. Index routing remains outside this operation, while the
    /// canonical header match and trait proof remain shared.
    pub fn matches_for_receiver_from_impls(
        &self,
        receiver_ty: &Ty,
        inherent_impls: UniqueVec<ImplRef>,
        trait_impls: UniqueVec<TraitImplRef>,
    ) -> Result<ReceiverImplMatches, D::Error> {
        let mut matches =
            self.inherent_matches_for_receiver_from_impls(receiver_ty, inherent_impls)?;
        matches
            .traits
            .extend(self.trait_selections_for_receiver_from_impls(receiver_ty, trait_impls)?);
        Ok(matches)
    }

    /// Match an explicit inherent impl set while retaining receiver substitutions.
    fn inherent_matches_for_receiver_from_impls(
        &self,
        receiver_ty: &Ty,
        inherent_impls: impl IntoIterator<Item = ImplRef>,
    ) -> Result<ReceiverImplMatches, D::Error> {
        let item_query = self.context.item_paths().items();
        let mut matches = ReceiverImplMatches::default();

        for impl_ref in inherent_impls {
            if self.context.cancellation().is_cancelled() {
                return Ok(Default::default());
            }
            let Some(impl_data) = item_query.impl_data(impl_ref)? else {
                continue;
            };
            if impl_data.trait_ref.is_some() {
                continue;
            }
            let Some(selected) = crate::trait_selection::TraitSelectionQuery::with_resolver(
                self.context.clone(),
                &self.resolver,
            )
            .select_impl(impl_ref, receiver_ty)?
            else {
                continue;
            };
            let subst = selected.subst;
            let applicability = selected.applicability;
            if !applicability.is_applicable() {
                continue;
            }
            let candidate = InherentImplMatch {
                impl_ref,
                subst,
                applicability,
            };
            matches.inherent.push(candidate);
        }

        Ok(matches)
    }

    /// Expand matched impls into functions without deciding method-vs-associated-call syntax.
    pub fn function_candidates_for_matches(
        &self,
        matches: &ReceiverImplMatches,
        function_name: Option<&str>,
    ) -> Result<Vec<ReceiverFunctionCandidate>, D::Error> {
        let item_query = self.context.item_paths().items();
        let mut functions = Vec::new();

        for impl_match in matches.inherent() {
            if self.context.cancellation().is_cancelled() {
                return Ok(Default::default());
            }
            let Some(impl_data) = item_query.impl_data(impl_match.impl_ref())? else {
                continue;
            };
            for function in impl_data.functions() {
                if self.context.cancellation().is_cancelled() {
                    return Ok(Default::default());
                }
                if let Some(name) = function_name {
                    let Some(function_data) = item_query.function_data(function)? else {
                        continue;
                    };
                    if function_data.name != name {
                        continue;
                    }
                }
                functions.push(ReceiverFunctionCandidate {
                    function,
                    source: ReceiverFunctionSource::Inherent {
                        impl_match: impl_match.clone(),
                    },
                });
            }
        }

        for selection in matches.traits() {
            if self.context.cancellation().is_cancelled() {
                return Ok(Default::default());
            }
            let trait_ref = selection.trait_impl.trait_ref;
            let named = match function_name {
                Some(name) => self
                    .context
                    .item_lookup()
                    .trait_functions_by_name(trait_ref, name),
                None => Ok(None),
            };
            let Ok(named) = named else {
                return Ok(Vec::new());
            };
            let indexed = match named {
                Some(functions) => Some(functions),
                None => {
                    let Ok(functions) = self.context.item_lookup().trait_functions(trait_ref)
                    else {
                        return Ok(Vec::new());
                    };
                    functions
                }
            };
            let trait_functions = if let Some(functions) = indexed {
                functions
            } else {
                // Saved-project traits are guaranteed to be present in the semantic lookup query.
                // A current-body trait deliberately is not: its declaration lives alongside the
                // local impl selected by the body overlay.
                if !matches!(trait_ref.origin, DefMapRef::Body(_)) {
                    continue;
                }
                item_query
                    .trait_data(trait_ref)?
                    .map(|trait_data| trait_data.functions().collect())
                    .unwrap_or_default()
            };

            for function in trait_functions {
                if self.context.cancellation().is_cancelled() {
                    return Ok(Default::default());
                }
                if let Some(name) = function_name {
                    let Some(function_data) = item_query.function_data(function)? else {
                        continue;
                    };
                    if function_data.name != name {
                        continue;
                    }
                }

                // Keep the selected impl with the function. Two overlapping impls can expose the
                // same declaration but carry different proof evidence; collapsing them by
                // function identity would make an ambiguous call look uniquely selected.
                functions.push(ReceiverFunctionCandidate {
                    function,
                    source: ReceiverFunctionSource::Trait {
                        selection: selection.clone(),
                    },
                });
            }
        }

        Ok(functions)
    }
}
