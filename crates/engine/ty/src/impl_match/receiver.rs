//! Receiver-centric impl discovery.
//!
//! The item index has two useful storage shapes: nominal impls are keyed by `TypeDefRef`, while
//! structural and blanket impls live in compact fallback lists. That distinction stops here. A
//! caller asks which impls match one receiver and receives the evidence needed to instantiate any
//! item selected from those impls.

use rg_def_map::DefMapSource;
use rg_ir_model::{DefMapRef, FunctionRef, ImplRef, TraitApplicability, TraitImplRef};
use rg_item_tree::LangItem;
use rg_semantic_ir::ItemStoreSource;
use rg_std::UniqueVec;

use crate::{
    Clause, Substitution, TraitSelection, Ty, TypePathResolver, inference::InferenceTable,
};

use super::ImplMatcher;

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
/// Inherent matches retain owner substitutions. Trait matches retain the stronger
/// [`TraitSelection`] result because it also carries the instantiated trait arguments and the
/// trial inference table. Consumers can therefore adapt the same evidence into methods,
/// associated functions, constants, or completion declarations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReceiverImplMatches {
    inherent: Vec<InherentImplMatch>,
    traits: Vec<TraitSelection>,
}

impl ReceiverImplMatches {
    pub fn inherent(&self) -> &[InherentImplMatch] {
        &self.inherent
    }

    pub fn traits(&self) -> &[TraitSelection] {
        &self.traits
    }

    /// Append another visible impl universe without collapsing distinct proof results.
    ///
    /// Body lookup uses this to place current-body matches before saved-project matches. Exact
    /// duplicates can arise when two lookup routes expose the same impl, but overlapping trait
    /// impls with different selections remain separate candidates.
    pub fn extend(&mut self, other: Self) {
        for candidate in other.inherent {
            if !self.inherent.contains(&candidate) {
                self.inherent.push(candidate);
            }
        }
        for selection in other.traits {
            if !self.traits.contains(&selection) {
                self.traits.push(selection);
            }
        }
    }
}

/// One function declaration together with the impl evidence that exposed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiverFunctionCandidate {
    function: FunctionRef,
    source: ReceiverFunctionSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReceiverFunctionSource {
    Inherent { impl_match: InherentImplMatch },
    Trait { selection: TraitSelection },
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

    pub fn trait_selection(&self) -> Option<&TraitSelection> {
        match &self.source {
            ReceiverFunctionSource::Trait { selection } => Some(selection),
            ReceiverFunctionSource::Inherent { .. } => None,
        }
    }
}

impl<'query, D, I, R> ImplMatcher<'query, D, I, R>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    /// Match every saved-project impl that can apply to this receiver.
    ///
    /// `Widget` uses the nominal indexes and also checks blanket trait impls such as
    /// `impl<T> Describe for T`. `[Widget]` uses the structural inherent list and the same trait
    /// fallback. Callers never need to branch on those storage categories themselves.
    pub fn matches_for_receiver(
        &self,
        receiver_ty: &Ty,
        table: &InferenceTable,
    ) -> Result<ReceiverImplMatches, D::Error> {
        let mut inherent_impls = UniqueVec::new();
        let mut trait_impls = UniqueVec::new();
        for receiver in receiver_ty.as_adts() {
            inherent_impls.extend(
                self.context
                    .item_lookup()
                    .inherent_impls_for_type(receiver.def),
            );
            trait_impls.extend(
                self.context
                    .item_lookup()
                    .trait_impls_for_type(receiver.def),
            );
        }

        let mut matches =
            self.matches_for_receiver_from_impls(receiver_ty, inherent_impls, trait_impls, table)?;

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
            // Builtin-shaped inherent impls need stricter predicate handling than indexed nominal
            // impls. `PointeeSized` has no ordinary impl declarations: rustc provides it for every
            // type that can occur behind a pointer. Core writes `impl<T: PointeeSized> *const T`
            // for the main raw-pointer methods, so this is the one predicate we can establish from
            // compiler identity alone. Other unresolved predicates must not expose items
            // tentatively.
            let pointee_sized = self
                .context
                .item_lookup()
                .lang_trait(LangItem::PointeeSized);
            for impl_ref in self.context.item_lookup().structural_inherent_impls() {
                let Some(impl_data) = self.context.item_paths().items().impl_data(impl_ref)? else {
                    continue;
                };
                if impl_data.trait_ref.is_some() {
                    continue;
                }
                let Some(header) = self.impl_header(impl_ref)? else {
                    continue;
                };
                if header.clauses.iter().any(|clause| {
                    !matches!(
                        clause,
                        Clause::Implemented(application)
                            if Some(application.def) == pointee_sized
                    )
                }) {
                    continue;
                }
                let Some((subst, applicability)) = Self::impl_self_subst(&header, receiver_ty)
                else {
                    continue;
                };
                if applicability != TraitApplicability::Yes {
                    continue;
                }
                let candidate = InherentImplMatch {
                    impl_ref,
                    subst,
                    applicability: TraitApplicability::Yes,
                };
                if !matches.inherent.contains(&candidate) {
                    matches.inherent.push(candidate);
                }
            }
        }

        // The fallback trait bucket also contains blanket impls, so it participates for nominal
        // receivers as well as primitives and structural types.
        let fallback = self.matches_for_receiver_from_impls(
            receiver_ty,
            UniqueVec::new(),
            self.context.item_lookup().trait_impls_without_type_key(),
            table,
        )?;
        matches.extend(fallback);
        Ok(matches)
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
        table: &InferenceTable,
    ) -> Result<ReceiverImplMatches, D::Error> {
        let item_query = self.context.item_paths().items();
        let mut matches = ReceiverImplMatches::default();

        for impl_ref in inherent_impls {
            let Some(impl_data) = item_query.impl_data(impl_ref)? else {
                continue;
            };
            if impl_data.trait_ref.is_some() {
                continue;
            }
            let Some((subst, applicability)) =
                self.impl_self_subst_for_impl(impl_ref, receiver_ty)?
            else {
                continue;
            };
            if !applicability.is_applicable() {
                continue;
            }
            let candidate = InherentImplMatch {
                impl_ref,
                subst,
                applicability,
            };
            if !matches.inherent.contains(&candidate) {
                matches.inherent.push(candidate);
            }
        }

        for trait_impl in trait_impls {
            let Some(selection) =
                self.trait_impl_selection_for_ty(trait_impl, receiver_ty, table)?
            else {
                continue;
            };
            if !matches.traits.contains(&selection) {
                matches.traits.push(selection);
            }
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
            let Some(impl_data) = item_query.impl_data(impl_match.impl_ref())? else {
                continue;
            };
            for function in impl_data.functions() {
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
            let trait_ref = selection.trait_impl.trait_ref;
            let trait_functions = if let Some(name) = function_name
                && let Some(functions) = self
                    .context
                    .item_lookup()
                    .trait_functions_by_name(trait_ref, name)
            {
                functions
            } else if let Some(functions) = self.context.item_lookup().trait_functions(trait_ref) {
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
