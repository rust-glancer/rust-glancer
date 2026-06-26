//! Best-effort trait impl matching for method candidates.
//!
//! Editor-facing method lookup can preserve useful `Maybe` candidates when a proof would require
//! deeper solving. Simple direct cases still reuse bounded trait selection for consistency.

use crate::inference::{InferTy, InferenceTable};
use crate::{GenericArg, NominalTy, TraitGoal, TraitSelectionQuery, Ty, TypeSubst};
use rg_ir_model::hir::items::ImplData;
use rg_ir_model::items::{GenericArg as ItemGenericArg, TypeRef};
use rg_ir_model::{FunctionRef, ImplRef, TraitApplicability, TraitImplRef};
use rg_ir_storage::{DefMapSource, ItemLookupIndex, ItemStoreSource, TypePathContext};
use rg_std::UniqueVec;

use super::ImplMatcher;

/// Result of matching one trait impl header against a receiver type.
struct TraitImplMatch {
    applicability: TraitApplicability,
    subst: TypeSubst,
}

impl TraitImplMatch {
    /// Creates a match result from the computed confidence and receiver substitutions.
    fn new(applicability: TraitApplicability, subst: TypeSubst) -> Self {
        Self {
            applicability,
            subst,
        }
    }

    /// Confidence that the impl header applies to the receiver.
    fn applicability(&self) -> TraitApplicability {
        self.applicability
    }

    /// Splits the result into the match confidence and substitutions for associated signatures.
    fn into_parts(self) -> (TraitApplicability, TypeSubst) {
        (self.applicability, self.subst)
    }
}

impl<'query, D, I> ImplMatcher<'query, D, I>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
{
    /// Returns only the yes/maybe/no part of `trait_impl_match`.
    pub fn trait_impl_applicability(
        &self,
        trait_impl: TraitImplRef,
        receiver_ty: &NominalTy,
    ) -> Result<TraitApplicability, D::Error> {
        Ok(self
            .trait_impl_match(trait_impl, receiver_ty)?
            .map(|trait_impl_match| trait_impl_match.applicability())
            .unwrap_or(TraitApplicability::No))
    }

    /// Returns trait-associated function candidates for a nominal receiver.
    pub fn trait_function_candidates_for_receiver(
        &self,
        index: &ItemLookupIndex,
        receiver_ty: &NominalTy,
        method_name: Option<&str>,
    ) -> Result<Vec<(FunctionRef, TraitApplicability)>, D::Error> {
        let trait_impls = index
            .trait_impls_for_type(receiver_ty.def)
            .cloned()
            .unwrap_or_default();
        self.trait_function_candidates_from_impls(index, trait_impls, receiver_ty, method_name)
    }

    /// Expands already-collected trait impl candidates into trait function candidates.
    ///
    /// The caller owns visibility and overlay rules by deciding which trait impl refs to pass in;
    /// this method owns only receiver applicability and trait-associated function expansion.
    pub fn trait_function_candidates_from_impls(
        &self,
        index: &ItemLookupIndex,
        trait_impls: UniqueVec<TraitImplRef>,
        receiver_ty: &NominalTy,
        method_name: Option<&str>,
    ) -> Result<Vec<(FunctionRef, TraitApplicability)>, D::Error> {
        let item_query = self.item_paths.items();
        let mut functions = Vec::new();
        for trait_impl in trait_impls {
            // For method calls, the name is known before we do any trait-impl compatibility work.
            // If the indexed trait has no function with that name, this impl cannot contribute a
            // candidate regardless of how well the impl header matches the receiver.
            let mut indexed_trait_functions = None;
            if let Some(method_name) = method_name
                && let Some(indexed_functions) =
                    index.trait_functions_by_name(trait_impl.trait_ref, method_name)
            {
                if indexed_functions.is_empty() {
                    continue;
                }
                indexed_trait_functions = indexed_functions.functions().cloned();
            }

            let Some(trait_impl_match) = self.trait_impl_match(trait_impl, receiver_ty)? else {
                continue;
            };
            let (applicability, _) = trait_impl_match.into_parts();

            let trait_functions = if let Some(functions) = indexed_trait_functions {
                functions
            } else if let Some(functions) = index.trait_functions(trait_impl.trait_ref) {
                functions.clone()
            } else {
                let trait_functions = item_query
                    .trait_data(trait_impl.trait_ref)?
                    .map(|t| t.functions().collect())
                    .unwrap_or_default();

                // The direct item-store fallback cannot skip the impl check up front, but it can
                // still avoid returning unrelated trait functions to the later method-call filter.
                if let Some(method_name) = method_name {
                    let mut retained = UniqueVec::new();
                    for function in trait_functions {
                        let Some(function_data) = item_query.function_data(function)? else {
                            continue;
                        };
                        if function_data.name == method_name {
                            retained.push(function);
                        }
                    }
                    retained
                } else {
                    trait_functions
                }
            };

            for function in trait_functions {
                Self::push_function_candidate(&mut functions, function, applicability);
            }
        }

        Ok(functions)
    }

    /// Matches one trait impl against a receiver.
    ///
    /// For `impl<T> Trait for Wrapper<T>` and receiver `Wrapper<User>`, this returns an
    /// `TraitImplMatch` whose substitutions include `T -> User`.
    fn trait_impl_match(
        &self,
        trait_impl: TraitImplRef,
        receiver_ty: &NominalTy,
    ) -> Result<Option<TraitImplMatch>, D::Error> {
        let item_query = self.item_paths.items();
        let Some(impl_data) = item_query.impl_data(trait_impl.impl_ref)? else {
            return Ok(None);
        };
        if !impl_data.resolved_self_ty.is(&receiver_ty.def)
            || !impl_data.resolved_trait_ref.is(&trait_impl.trait_ref)
        {
            return Ok(None);
        }

        let header_applicability = Self::impl_header_applicability(impl_data);
        if Self::trait_selection_can_check_method_lookup_impl(impl_data, receiver_ty) {
            return self.trait_selection_impl_match(
                trait_impl,
                impl_data,
                receiver_ty,
                header_applicability,
            );
        }

        let applicability = header_applicability.and(self.impl_self_args_applicability(
            trait_impl.impl_ref,
            impl_data,
            receiver_ty,
        )?);
        if !applicability.is_applicable() {
            return Ok(None);
        }

        Ok(Some(TraitImplMatch::new(
            applicability,
            self.impl_self_subst_for_impl(impl_data, receiver_ty),
        )))
    }

    /// Reuse bounded trait selection for the receiver-proof part of trait method lookup.
    ///
    /// Method lookup starts from visible impls and only asks whether their self type can apply to a
    /// receiver. Generic trait arguments are not caller-supplied here, so this bridge is limited to
    /// non-generic trait paths and direct/concrete receiver headers. More uncertain headers keep
    /// using the older maybe-applicable path below.
    fn trait_selection_impl_match(
        &self,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        receiver_ty: &NominalTy,
        header_applicability: TraitApplicability,
    ) -> Result<Option<TraitImplMatch>, D::Error> {
        let goal = TraitGoal {
            self_ty: InferTy::from_ty(&Ty::nominal(receiver_ty.clone())),
            trait_ref: trait_impl.trait_ref,
            args: Vec::new(),
        };
        let table = InferenceTable::new();
        let Some(selection) = TraitSelectionQuery::probe_visible_trait_impl(
            &self.item_paths,
            &self.target_items,
            &goal,
            &table,
            trait_impl,
        )?
        else {
            return Ok(None);
        };

        Ok(Some(TraitImplMatch::new(
            header_applicability.and(selection.applicability),
            self.impl_self_subst_for_impl(impl_data, receiver_ty),
        )))
    }

    /// Performs the trait-impl self-type argument check with uncertainty preserved.
    ///
    /// Generic or syntax-only pieces produce `Maybe` instead of being rejected, because callers can
    /// still show useful trait-method candidates when a full proof is outside this small model.
    fn impl_self_args_applicability(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        receiver_ty: &NominalTy,
    ) -> Result<TraitApplicability, D::Error> {
        // This mirrors inherent impl matching, but returns `Maybe` instead of rejecting patterns
        // that contain generic parameters or unsupported pieces we intentionally do not solve.
        let TypeRef::Path(self_ty) = &impl_data.self_ty else {
            return Ok(TraitApplicability::Maybe);
        };
        let Some(segment) = self_ty.segments.last() else {
            return Ok(TraitApplicability::Maybe);
        };

        let Some(impl_type_args) = Self::item_tree_type_args(&segment.args) else {
            return Ok(TraitApplicability::Maybe);
        };
        let Some(receiver_type_args) = Self::ty_args(&receiver_ty.args) else {
            return Ok(TraitApplicability::Maybe);
        };
        if impl_type_args.len() != receiver_type_args.len() {
            return Ok(TraitApplicability::Maybe);
        }

        let impl_type_params = Self::impl_type_param_names(&impl_data.generics);
        let mut applicability = TraitApplicability::Yes;

        for (impl_arg, receiver_arg) in impl_type_args.into_iter().zip(receiver_type_args) {
            if impl_arg.mentions_type_param(&impl_type_params) {
                applicability = applicability.and(TraitApplicability::Maybe);
                continue;
            }

            let context = TypePathContext {
                module: impl_data.owner,
                impl_ref: Some(impl_ref),
            };
            let impl_arg_ty = self.item_paths.resolve_type_ref(
                impl_arg,
                context,
                Ty::syntax(impl_arg.clone()),
                &TypeSubst::new(),
            )?;
            if Self::type_arg_comparison_is_uncertain(&impl_arg_ty)
                || Self::type_arg_comparison_is_uncertain(&receiver_arg)
            {
                applicability = applicability.and(TraitApplicability::Maybe);
                continue;
            }

            if impl_arg_ty != receiver_arg {
                return Ok(TraitApplicability::No);
            }
        }

        Ok(applicability)
    }

    /// Return whether method lookup can use bounded trait selection without losing maybe-results.
    fn trait_selection_can_check_method_lookup_impl(
        impl_data: &ImplData,
        receiver_ty: &NominalTy,
    ) -> bool {
        let header_is_supported = impl_data.generics.where_predicates.is_empty()
            && impl_data
                .generics
                .lifetimes
                .iter()
                .all(|param| param.bounds.is_empty())
            && impl_data
                .generics
                .types
                .iter()
                .all(|param| param.bounds.is_empty() && param.default.is_none())
            && impl_data.generics.consts.is_empty()
            && impl_data
                .trait_ref
                .as_ref()
                .is_some_and(|trait_ref| !trait_ref.has_generic_args());
        if !header_is_supported {
            return false;
        }

        let TypeRef::Path(self_ty) = &impl_data.self_ty else {
            return false;
        };
        let Some(segment) = self_ty.segments.last() else {
            return false;
        };
        if segment.args.len() != receiver_ty.args.len() {
            return false;
        }

        let impl_type_params = Self::impl_type_param_names(&impl_data.generics);
        segment
            .args
            .iter()
            .zip(&receiver_ty.args)
            .all(|(impl_arg, receiver_arg)| {
                Self::trait_selection_can_check_method_lookup_arg(
                    impl_arg,
                    receiver_arg,
                    &impl_type_params,
                )
            })
    }

    /// Keep consts, assoc bindings, and nested generic patterns on the maybe-compatible path.
    fn trait_selection_can_check_method_lookup_arg(
        impl_arg: &ItemGenericArg,
        receiver_arg: &GenericArg,
        impl_type_params: &[&str],
    ) -> bool {
        match (impl_arg, receiver_arg) {
            (ItemGenericArg::Lifetime(_), GenericArg::Lifetime(_)) => true,
            (ItemGenericArg::Type(impl_ty), GenericArg::Type(_)) => {
                impl_ty
                    .type_param_name()
                    .as_deref()
                    .is_some_and(|name| impl_type_params.contains(&name))
                    || !impl_ty.mentions_type_param(impl_type_params)
            }
            _ => false,
        }
    }

    fn push_function_candidate(
        functions: &mut Vec<(FunctionRef, TraitApplicability)>,
        function: FunctionRef,
        applicability: TraitApplicability,
    ) {
        if let Some((_, existing)) = functions
            .iter_mut()
            .find(|(existing_function, _)| *existing_function == function)
        {
            *existing = existing.or(applicability);
            return;
        }

        functions.push((function, applicability));
    }
}
