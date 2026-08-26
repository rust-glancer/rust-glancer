//! Method lookup for receiver types.
//!
//! Ref-level member lookup can stop after identifying a function. Body inference also keeps the
//! receiver substitutions and trait-selection evidence needed to instantiate its signature. For
//! `[User; 3].into_iter()`, the selected array impl carries `T = User` and `N = 3`, allowing the
//! return projection to become `array::IntoIter<User, 3>`.

use rg_def_map::DefMapSource;
use rg_ir_model::{FunctionRef, ItemOwner, TraitImplRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::UniqueVec;
use rg_ty::{
    AdtTy, AutoderefMode, ImplMatcher, MemberMethodCandidateRef, MemberMethodOrigin, Substitution,
    TraitSelection, Ty, inference::InferenceTable,
};

use crate::resolution::{BodyQuerySource, BodyResolutionContext};

use super::BodyLocalItemQuery;

type BodyImplMatcher<'context, 'query, D, I> = ImplMatcher<
    'query,
    BodyQuerySource<'query, D, I>,
    BodyQuerySource<'query, D, I>,
    &'context BodyResolutionContext<'query, D, I>,
>;

/// Resolves methods for receiver types.
pub struct BodyMethodQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

/// Method candidate selected by receiver lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyMethodCandidate {
    function: FunctionRef,
    receiver_ty: Ty,
    subst: Substitution,
    trait_selection: Option<TraitSelection>,
}

/// Nominal method plus any trait proof retained until the caller supplies receiver substitutions.
struct NominalMethodCandidate {
    function: FunctionRef,
    trait_selection: Option<TraitSelection>,
}

impl BodyMethodCandidate {
    /// Return the selected method function.
    pub(crate) fn function(&self) -> FunctionRef {
        self.function
    }

    /// Return the receiver type used for this candidate.
    pub(crate) fn receiver_ty(&self) -> &Ty {
        &self.receiver_ty
    }

    /// Return substitutions derived from the receiver and impl owner.
    pub(crate) fn subst(&self) -> &Substitution {
        &self.subst
    }

    /// Return the trait-selection evidence that made this candidate available.
    ///
    /// Inherent candidates have no trait obligation and therefore no selection to commit. A
    /// `Maybe` selection may still be shown by editor lookup, but call inference cannot commit it.
    pub(crate) fn trait_selection(&self) -> Option<&TraitSelection> {
        self.trait_selection.as_ref()
    }
}

impl<'query, D, I> BodyMethodQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Return all methods that can be reached from this receiver type.
    pub fn method_candidates_for_ty(
        &self,
        ty: &Ty,
    ) -> Result<Vec<MemberMethodCandidateRef>, PackageStoreError> {
        let matcher = self.context.impl_matcher();
        let table = InferenceTable::new();
        let mut candidates = Vec::new();
        for candidate in self
            .context
            .autoderef()
            .candidates(AutoderefMode::MethodReceiver, ty)
        {
            let candidate = candidate?;
            for receiver_ty in candidate.ty().as_adts() {
                for method in self.nominal_method_candidates(&matcher, receiver_ty, None, &table)? {
                    let candidate = match &method.trait_selection {
                        Some(selection) => MemberMethodCandidateRef::trait_method(
                            method.function,
                            selection.applicability,
                        ),
                        None => MemberMethodCandidateRef::inherent(method.function),
                    };
                    Self::push_candidate(&mut candidates, candidate);
                }
            }
            for method in self.unkeyed_method_candidates(&matcher, candidate.ty(), None, &table)? {
                let candidate = match &method.trait_selection {
                    Some(selection) => MemberMethodCandidateRef::trait_method(
                        method.function,
                        selection.applicability,
                    ),
                    None => MemberMethodCandidateRef::inherent(method.function),
                };
                Self::push_candidate(&mut candidates, candidate);
            }
        }

        Ok(candidates)
    }

    /// Return named method candidates at the first matching autoderef depth.
    pub(crate) fn named_method_candidates_for_ty(
        &self,
        receiver_ty: &Ty,
        method_name: &str,
        table: &InferenceTable,
    ) -> Result<Vec<BodyMethodCandidate>, PackageStoreError> {
        let item_query = self.context.item_query();
        let matcher = self.context.impl_matcher();
        let mut current_depth = None;
        let mut candidates = Vec::new();

        for candidate in self
            .context
            .autoderef()
            .candidates(AutoderefMode::MethodReceiver, receiver_ty)
        {
            let candidate = candidate?;
            // Method calls select the first autoderef depth that has matching methods. Completion
            // can be more generous, but call inference must not mix receiver substitutions across
            // different depths.
            if current_depth.is_some_and(|depth| depth != candidate.depth())
                && !candidates.is_empty()
            {
                return Ok(candidates);
            }
            current_depth = Some(candidate.depth());

            for nominal_ty in candidate.ty().as_adts() {
                for method in
                    self.nominal_method_candidates(&matcher, nominal_ty, Some(method_name), table)?
                {
                    let function_ref = method.function;
                    let Some(function_data) = item_query.function_data(function_ref)? else {
                        continue;
                    };
                    if function_data.name != method_name || !function_data.has_self_receiver() {
                        continue;
                    }

                    candidates.push(BodyMethodCandidate {
                        function: function_ref,
                        receiver_ty: Ty::adt(nominal_ty.clone()),
                        subst: self.nominal_method_subst(
                            function_ref,
                            function_data.owner,
                            nominal_ty,
                        )?,
                        trait_selection: method.trait_selection,
                    });
                }
            }

            for structural in
                self.unkeyed_method_candidates(&matcher, candidate.ty(), Some(method_name), table)?
            {
                let Some(function_data) = item_query.function_data(structural.function)? else {
                    continue;
                };
                if function_data.name != method_name || !function_data.has_self_receiver() {
                    continue;
                }

                candidates.push(BodyMethodCandidate {
                    function: structural.function,
                    receiver_ty: structural.receiver_ty,
                    subst: structural.subst,
                    trait_selection: structural.trait_selection,
                });
            }
        }

        Ok(candidates)
    }

    /// Collect inherent and trait methods for a nominal receiver.
    fn nominal_method_candidates(
        &self,
        matcher: &BodyImplMatcher<'_, 'query, D, I>,
        receiver_ty: &AdtTy,
        method_name: Option<&str>,
        table: &InferenceTable,
    ) -> Result<Vec<NominalMethodCandidate>, PackageStoreError> {
        let body_items = self.context.body_local_items();
        let body_inherent_names = body_items.inherent_function_names_for_type(receiver_ty.def)?;
        let mut candidates = Vec::new();

        for function in self.body_inherent_functions(&body_items, receiver_ty, method_name)? {
            if matcher.function_applies_to_receiver(function, receiver_ty)? {
                candidates.push(NominalMethodCandidate {
                    function,
                    trait_selection: None,
                });
            }
        }

        if receiver_ty.def.origin.as_crate_ref().is_some() {
            for function in self.semantic_inherent_functions(receiver_ty, method_name)? {
                if self
                    .context
                    .item_query()
                    .function_data(function)?
                    .is_some_and(|data| body_inherent_names.contains(&data.name))
                {
                    continue;
                }
                if matcher.function_applies_to_receiver(function, receiver_ty)? {
                    candidates.push(NominalMethodCandidate {
                        function,
                        trait_selection: None,
                    });
                }
            }
        }

        let body_trait_impls = body_items.trait_impls_for_type(receiver_ty.def)?;
        let body_trait_functions = matcher.trait_function_candidates_from_impls(
            body_trait_impls,
            receiver_ty,
            method_name,
            table,
        )?;
        for (function, selection) in body_trait_functions {
            candidates.push(NominalMethodCandidate {
                function,
                trait_selection: Some(selection),
            });
        }

        if receiver_ty.def.origin.as_crate_ref().is_some() {
            let semantic_trait_functions =
                matcher.trait_function_candidates_for_receiver(receiver_ty, method_name, table)?;
            for (function, selection) in semantic_trait_functions {
                candidates.push(NominalMethodCandidate {
                    function,
                    trait_selection: Some(selection),
                });
            }
        }

        // Blanket impls such as `impl<T> Trait for T` have no nominal receiver key. They can
        // still apply to this ADT, so run the compact fallback list through canonical matching.
        let receiver_ty = Ty::adt(receiver_ty.clone());
        let fallback_trait_functions = matcher.trait_function_candidates_from_impls_for_ty(
            self.trait_impls_without_type_key(&body_items)?,
            &receiver_ty,
            method_name,
            table,
        )?;
        for (function, selection) in fallback_trait_functions {
            candidates.push(NominalMethodCandidate {
                function,
                trait_selection: Some(selection),
            });
        }

        Ok(candidates)
    }

    /// Scan visible impls for builtin-shaped receiver types.
    ///
    /// A receiver such as `[User; 3]` has no nominal index entry. Its inherent candidates come
    /// from structural impl matching, while trait candidates come from the bounded unkeyed impl
    /// list. The inherent path retains its substitution directly; the trait path retains the
    /// selected impl and its bindings as trait-selection evidence.
    fn unkeyed_method_candidates(
        &self,
        matcher: &BodyImplMatcher<'_, 'query, D, I>,
        receiver_ty: &Ty,
        method_name: Option<&str>,
        table: &InferenceTable,
    ) -> Result<Vec<BodyMethodCandidate>, PackageStoreError> {
        // Nominal receivers are handled by the indexed path. Scanning visible impls is reserved
        // for builtin types such as `str` and `[T]`, where there is no `TypeDefRef` key to query.
        if !receiver_ty.has_unkeyed_self_head() {
            return Ok(Vec::new());
        }

        let item_query = self.context.item_query();
        let mut candidates = Vec::new();

        // Unkeyed inherent impls model language/core-provided builtins such as `impl<T> [T]`.
        // Body-local impl lookup remains nominal-only because block-local impls are useful for
        // local structs, not for defining new inherent methods on builtin shaped types.
        for (function, subst) in
            matcher.unkeyed_inherent_function_candidates(receiver_ty, method_name)?
        {
            Self::push_unkeyed_candidate(
                &mut candidates,
                BodyMethodCandidate {
                    function,
                    receiver_ty: receiver_ty.clone(),
                    subst,
                    trait_selection: None,
                },
            );
        }

        // Trait impls for primitives, arrays, slices, and generic `Self` types share the same
        // absence of a nominal index key. Canonical matching separates the applicable receiver
        // shapes and classifies each impl's predicates before its trait functions become
        // candidates.
        let body_items = self.context.body_local_items();
        let trait_functions = matcher.trait_function_candidates_from_impls_for_ty(
            self.trait_impls_without_type_key(&body_items)?,
            receiver_ty,
            method_name,
            table,
        )?;
        for (function, selection) in trait_functions {
            let Some(function_data) = item_query.function_data(function)? else {
                continue;
            };
            if !function_data.has_self_receiver() {
                continue;
            }
            Self::push_unkeyed_candidate(
                &mut candidates,
                BodyMethodCandidate {
                    function,
                    receiver_ty: receiver_ty.clone(),
                    subst: self.context.generics().subst_for_receiver_ty_owner(
                        function.origin,
                        function_data.owner,
                        receiver_ty,
                    )?,
                    trait_selection: Some(selection),
                },
            );
        }

        Ok(candidates)
    }

    /// Combine unkeyed trait impls from body overlays and persisted visible crates.
    fn trait_impls_without_type_key(
        &self,
        body_items: &BodyLocalItemQuery<'query, D, I>,
    ) -> Result<UniqueVec<TraitImplRef>, PackageStoreError> {
        let mut trait_impls = body_items.trait_impls_without_type_key()?;
        trait_impls.extend(
            self.context
                .item_lookup_query()
                .trait_impls_without_type_key(),
        );
        Ok(trait_impls)
    }

    /// Read body-local inherent functions, optionally filtered by name.
    fn body_inherent_functions(
        &self,
        body_items: &BodyLocalItemQuery<'query, D, I>,
        receiver_ty: &AdtTy,
        method_name: Option<&str>,
    ) -> Result<UniqueVec<FunctionRef>, PackageStoreError> {
        let functions = body_items.inherent_functions_for_type(receiver_ty.def)?;
        self.filter_functions_by_name(functions, method_name)
    }

    /// Read crate-visible inherent functions, optionally filtered by name.
    fn semantic_inherent_functions(
        &self,
        receiver_ty: &AdtTy,
        method_name: Option<&str>,
    ) -> Result<UniqueVec<FunctionRef>, PackageStoreError> {
        let index = self.context.item_lookup_query();
        match method_name {
            Some(name) => Ok(index.inherent_functions_for_type_and_name(receiver_ty.def, name)),
            None => {
                let item_query = self.context.item_query();
                index.inherent_functions_for_type(&item_query, receiver_ty.def)
            }
        }
    }

    /// Build receiver subst for a nominal method candidate.
    fn nominal_method_subst(
        &self,
        function_ref: FunctionRef,
        owner: ItemOwner,
        receiver_ty: &AdtTy,
    ) -> Result<Substitution, PackageStoreError> {
        self.context
            .generics()
            .subst_for_receiver_owner(function_ref.origin, owner, receiver_ty)
    }

    /// Keep functions whose item data has the requested name.
    fn filter_functions_by_name(
        &self,
        functions: UniqueVec<FunctionRef>,
        name: Option<&str>,
    ) -> Result<UniqueVec<FunctionRef>, PackageStoreError> {
        let Some(name) = name else {
            return Ok(functions);
        };

        let item_query = self.context.item_query();
        let mut retained = UniqueVec::new();
        for function in functions {
            let Some(function_data) = item_query.function_data(function)? else {
                continue;
            };
            if function_data.name == name {
                retained.push(function);
            }
        }
        Ok(retained)
    }

    /// Deduplicate a method candidate and keep the stronger origin.
    fn push_candidate(
        candidates: &mut Vec<MemberMethodCandidateRef>,
        candidate: MemberMethodCandidateRef,
    ) {
        let Some(existing) = candidates
            .iter_mut()
            .find(|existing| existing.function() == candidate.function())
        else {
            candidates.push(candidate);
            return;
        };

        *existing = Self::merge_candidates(*existing, candidate);
    }

    /// Deduplicate a structural candidate without collapsing distinct trait proofs.
    fn push_unkeyed_candidate(
        candidates: &mut Vec<BodyMethodCandidate>,
        candidate: BodyMethodCandidate,
    ) {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }

    /// Merge duplicate candidates from inherent and trait lookup.
    fn merge_candidates(
        left: MemberMethodCandidateRef,
        right: MemberMethodCandidateRef,
    ) -> MemberMethodCandidateRef {
        match (left.origin(), right.origin()) {
            (MemberMethodOrigin::Inherent, _) => left,
            (_, MemberMethodOrigin::Inherent) => right,
            (
                MemberMethodOrigin::Trait {
                    applicability: left_applicability,
                },
                MemberMethodOrigin::Trait {
                    applicability: right_applicability,
                },
            ) => MemberMethodCandidateRef::trait_method(
                left.function(),
                left_applicability.or(right_applicability),
            ),
        }
    }
}
