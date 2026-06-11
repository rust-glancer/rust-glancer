//! Method lookup for receiver types.

use rg_ir_model::{AssocItemId, FunctionRef, ImplRef, ItemOwner};
use rg_ir_storage::{DefMapSource, ItemStoreQuery, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_std::UniqueVec;
use rg_ty::{
    AutoderefMode, ImplMatcher, MemberMethodCandidateRef, MemberMethodOrigin, NominalTy, Ty,
    TypeSubst,
};

use crate::resolution::{BodyQuerySource, BodyResolutionContext};

use super::BodyLocalItemQuery;

/// Resolves methods for receiver types.
pub struct BodyMethodQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

/// Method candidate selected by receiver lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyMethodCandidate {
    function: FunctionRef,
    receiver_ty: Ty,
    subst: TypeSubst,
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
    pub(crate) fn subst(&self) -> &TypeSubst {
        &self.subst
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
        let mut candidates = Vec::new();
        for candidate in self
            .context
            .autoderef()
            .candidates(AutoderefMode::MethodReceiver, ty)
        {
            let candidate = candidate?;
            for receiver_ty in candidate.ty().as_nominals() {
                for method in self.nominal_method_candidates(receiver_ty, None)? {
                    Self::push_candidate(&mut candidates, method);
                }
            }
            for method in self.structural_method_candidates(candidate.ty(), None)? {
                Self::push_candidate(
                    &mut candidates,
                    MemberMethodCandidateRef::inherent(method.function()),
                );
            }
        }

        Ok(candidates)
    }

    /// Return named method candidates at the first matching autoderef depth.
    pub(crate) fn named_method_candidates_for_ty(
        &self,
        receiver_ty: &Ty,
        method_name: &str,
    ) -> Result<Vec<BodyMethodCandidate>, PackageStoreError> {
        let item_query = self.context.item_query();
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

            for nominal_ty in candidate.ty().as_nominals() {
                for method in self.nominal_method_candidates(nominal_ty, Some(method_name))? {
                    let function_ref = method.function();
                    let Some(function_data) = item_query.function_data(function_ref)? else {
                        continue;
                    };
                    if function_data.name != method_name || !function_data.has_self_receiver() {
                        continue;
                    }

                    candidates.push(BodyMethodCandidate {
                        function: function_ref,
                        receiver_ty: Ty::nominal([nominal_ty.clone()].into_iter().collect()),
                        subst: self.nominal_method_subst(
                            function_ref,
                            function_data.owner,
                            nominal_ty,
                        )?,
                    });
                }
            }

            for structural in
                self.structural_method_candidates(candidate.ty(), Some(method_name))?
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
                });
            }
        }

        Ok(candidates)
    }

    /// Collect inherent and trait methods for a nominal receiver.
    fn nominal_method_candidates(
        &self,
        receiver_ty: &NominalTy,
        method_name: Option<&str>,
    ) -> Result<Vec<MemberMethodCandidateRef>, PackageStoreError> {
        let matcher = self.context.impl_matcher();
        let body_items = self.context.body_local_items();
        let mut candidates = Vec::new();

        for function in self.body_inherent_functions(&body_items, receiver_ty, method_name)? {
            if matcher.function_applies_to_receiver(function, receiver_ty)? {
                Self::push_candidate(
                    &mut candidates,
                    MemberMethodCandidateRef::inherent(function),
                );
            }
        }

        if receiver_ty.def.origin.as_target_ref().is_some() {
            for function in self.semantic_inherent_functions(receiver_ty, method_name)? {
                if matcher.function_applies_to_receiver(function, receiver_ty)? {
                    Self::push_candidate(
                        &mut candidates,
                        MemberMethodCandidateRef::inherent(function),
                    );
                }
            }
        }

        let body_trait_impls = body_items.trait_impls_for_type(receiver_ty.def)?;
        for (function, applicability) in matcher.trait_function_candidates_from_impls(
            self.context.semantic_index(),
            body_trait_impls,
            receiver_ty,
            method_name,
        )? {
            Self::push_candidate(
                &mut candidates,
                MemberMethodCandidateRef::trait_method(function, applicability),
            );
        }

        if receiver_ty.def.origin.as_target_ref().is_some() {
            for (function, applicability) in matcher.trait_function_candidates_for_receiver(
                self.context.semantic_index(),
                receiver_ty,
                method_name,
            )? {
                Self::push_candidate(
                    &mut candidates,
                    MemberMethodCandidateRef::trait_method(function, applicability),
                );
            }
        }

        Ok(candidates)
    }

    /// Scan visible structural impls for builtin-shaped receiver types.
    fn structural_method_candidates(
        &self,
        receiver_ty: &Ty,
        method_name: Option<&str>,
    ) -> Result<Vec<BodyMethodCandidate>, PackageStoreError> {
        // Nominal receivers are handled by the indexed path. Scanning visible impls is reserved
        // for shaped builtin types such as `[T]`, where there is no `TypeDefRef` key to query.
        if !Self::receiver_ty_uses_structural_impl_lookup(receiver_ty) {
            return Ok(Vec::new());
        }

        let target_items = self.context.target_items();
        let matcher = self.context.impl_matcher();
        let item_query = self.context.item_query();
        let mut candidates = Vec::new();

        // Structural inherent impls model language/core-provided builtins such as `impl<T> [T]`.
        // Body-local impl lookup remains nominal-only because block-local impls are useful for
        // local structs, not for defining new inherent methods on builtin shaped types.
        let impl_refs = match self.context.semantic_index() {
            Some(index) => index.structural_inherent_impls().clone(),
            None => target_items.inherent_impls()?,
        };
        for impl_ref in impl_refs {
            self.push_structural_inherent_functions_for_impl(
                &item_query,
                &matcher,
                impl_ref,
                receiver_ty,
                method_name,
                &mut candidates,
            )?;
        }

        Ok(candidates)
    }

    /// Add self-receiver functions from one structural impl when it applies.
    fn push_structural_inherent_functions_for_impl(
        &self,
        item_query: &ItemStoreQuery<'query, BodyQuerySource<'query, D, I>>,
        matcher: &ImplMatcher<'query, BodyQuerySource<'query, D, I>, BodyQuerySource<'query, D, I>>,
        impl_ref: ImplRef,
        receiver_ty: &Ty,
        method_name: Option<&str>,
        candidates: &mut Vec<BodyMethodCandidate>,
    ) -> Result<(), PackageStoreError> {
        let Some(impl_data) = item_query.impl_data(impl_ref)? else {
            return Ok(());
        };
        let Some(subst) =
            matcher.structural_inherent_impl_subst(impl_ref, impl_data, receiver_ty)?
        else {
            return Ok(());
        };

        for item in &impl_data.items {
            let AssocItemId::Function(id) = item else {
                continue;
            };
            let function = FunctionRef {
                origin: impl_ref.origin,
                id: *id,
            };
            let Some(function_data) = item_query.function_data(function)? else {
                continue;
            };
            if !function_data.has_self_receiver() {
                continue;
            }
            if method_name.is_some_and(|name| function_data.name != name) {
                continue;
            }
            Self::push_structural_candidate(
                candidates,
                BodyMethodCandidate {
                    function,
                    receiver_ty: receiver_ty.clone(),
                    subst: subst.clone(),
                },
            );
        }

        Ok(())
    }

    /// Return whether this receiver has no nominal type-def key for impl lookup.
    fn receiver_ty_uses_structural_impl_lookup(ty: &Ty) -> bool {
        matches!(ty, Ty::Tuple(_) | Ty::Array { .. } | Ty::Slice(_))
    }

    /// Read body-local inherent functions, optionally filtered by name.
    fn body_inherent_functions(
        &self,
        body_items: &BodyLocalItemQuery<'query, D, I>,
        receiver_ty: &NominalTy,
        method_name: Option<&str>,
    ) -> Result<UniqueVec<FunctionRef>, PackageStoreError> {
        let functions = body_items.inherent_functions_for_type(receiver_ty.def)?;
        self.filter_functions_by_name(functions, method_name)
    }

    /// Read target-visible inherent functions, optionally filtered by name.
    fn semantic_inherent_functions(
        &self,
        receiver_ty: &NominalTy,
        method_name: Option<&str>,
    ) -> Result<UniqueVec<FunctionRef>, PackageStoreError> {
        match (self.context.semantic_index(), method_name) {
            (Some(index), Some(name)) => Ok(index
                .inherent_functions_for_type_and_name(receiver_ty.def, name)
                .cloned()
                .unwrap_or_default()),
            (Some(index), None) => {
                let item_query = self.context.item_query();
                index.inherent_functions_for_type(&item_query, receiver_ty.def)
            }
            (None, method_name) => {
                let functions = self
                    .context
                    .target_items()
                    .inherent_functions_for_type(receiver_ty.def)?;
                self.filter_functions_by_name(functions, method_name)
            }
        }
    }

    /// Build receiver subst for a nominal method candidate.
    fn nominal_method_subst(
        &self,
        function_ref: FunctionRef,
        owner: ItemOwner,
        receiver_ty: &NominalTy,
    ) -> Result<TypeSubst, PackageStoreError> {
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

    /// Deduplicate a structural candidate by function and subst.
    fn push_structural_candidate(
        candidates: &mut Vec<BodyMethodCandidate>,
        candidate: BodyMethodCandidate,
    ) {
        if !candidates.iter().any(|existing| {
            existing.function == candidate.function && existing.subst == candidate.subst
        }) {
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
