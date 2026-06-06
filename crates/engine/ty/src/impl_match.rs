//! Shallow impl matching for receiver-based item queries.
//!
//! The matchers own the small amount of generic reasoning used by method lookup and associated
//! items. They compare explicit impl self types against known receiver types and produce the
//! substitutions that make associated signatures readable in the receiver context.

use crate::{GenericArg, ItemPathQuery, NominalTy, Ty, TypeSubst};
use rg_ir_model::items::{GenericArg as ItemGenericArg, GenericParams, TypeRef};
use rg_ir_model::{
    FunctionRef, ImplRef, ItemOwner, TraitApplicability, TraitImplRef, hir::items::ImplData,
};
use rg_ir_storage::{
    DefMapSource, ItemLookupIndex, ItemStoreSource, TargetItemQuery, TypePathContext,
};
use rg_text::Name;

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

/// Matcher for impl headers stored in semantic-shaped item stores.
pub struct ImplMatcher<'query, D, I> {
    item_paths: ItemPathQuery<'query, D, I>,
    target_items: TargetItemQuery<'query, D, I>,
}

impl<'query, D, I> ImplMatcher<'query, D, I>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
{
    /// Creates a matcher over the same path/item routing used by type conversion.
    pub fn new(
        item_paths: ItemPathQuery<'query, D, I>,
        target_items: TargetItemQuery<'query, D, I>,
    ) -> Self {
        Self {
            item_paths,
            target_items,
        }
    }

    /// Checks whether a function owned by an inherent impl can be called on the receiver type.
    ///
    /// Trait functions are accepted here because the trait impl candidate already carries the
    /// receiver-specific filtering before a trait function reaches this point.
    pub fn function_applies_to_receiver(
        &self,
        function_ref: FunctionRef,
        receiver_ty: &NominalTy,
    ) -> Result<bool, D::Error> {
        // Trait items are shared by all impl candidates in the best-effort model. Inherent impl
        // items, however, must at least match the receiver's resolved self type.
        let item_query = self.item_paths.items();
        let Some(function_data) = item_query.function_data(function_ref)? else {
            return Ok(false);
        };
        let ItemOwner::Impl(impl_id) = function_data.owner else {
            return Ok(true);
        };
        let impl_ref = ImplRef {
            origin: function_ref.origin,
            id: impl_id,
        };
        let Some(impl_data) = item_query.impl_data(impl_ref)? else {
            return Ok(false);
        };
        self.impl_applies_to_receiver(impl_ref, impl_data, receiver_ty)
    }

    /// Checks whether an already-loaded inherent impl applies to a nominal receiver.
    ///
    /// Both target Semantic IR and body shadow item stores use `ImplData`, so the low-level
    /// receiver check should not care which store provided the impl.
    pub fn impl_applies_to_receiver(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        receiver_ty: &NominalTy,
    ) -> Result<bool, D::Error> {
        if !impl_data.resolved_self_tys.contains(&receiver_ty.def) {
            return Ok(false);
        }

        self.impl_self_args_match_receiver(impl_ref, impl_data, receiver_ty)
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
        if !impl_data.resolved_self_tys.contains(&receiver_ty.def)
            || !impl_data
                .resolved_trait_refs
                .contains(&trait_impl.trait_ref)
        {
            return Ok(None);
        }

        let header_applicability = if Self::impl_header_is_definitely_direct(impl_data) {
            TraitApplicability::Yes
        } else {
            TraitApplicability::Maybe
        };
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

    /// Matches one trait impl for contexts that perform a real type adjustment.
    ///
    /// This is stricter than method candidate matching: only direct impl type parameters such as
    /// `Wrapper<T>` are bindable. Nested generic patterns like `Wrapper<Option<T>>`, where clauses,
    /// bounded params, lifetimes, and const generics are rejected until a real solver exists.
    pub fn trait_impl_structural_match(
        &self,
        trait_impl: TraitImplRef,
        receiver_ty: &NominalTy,
    ) -> Result<Option<TypeSubst>, D::Error> {
        let item_query = self.item_paths.items();
        let Some(impl_data) = item_query.impl_data(trait_impl.impl_ref)? else {
            return Ok(None);
        };
        if !impl_data.resolved_self_tys.contains(&receiver_ty.def)
            || !impl_data
                .resolved_trait_refs
                .contains(&trait_impl.trait_ref)
        {
            return Ok(None);
        }

        self.impl_self_structural_subst(trait_impl.impl_ref, impl_data, receiver_ty)
    }

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

    /// Builds receiver substitutions from already-loaded impl data.
    pub fn impl_self_subst_for_impl(
        &self,
        impl_data: &ImplData,
        receiver_ty: &NominalTy,
    ) -> TypeSubst {
        Self::impl_self_subst(&impl_data.generics, &impl_data.self_ty, &receiver_ty.args)
    }

    /// Returns trait-associated function candidates for a nominal receiver.
    ///
    /// The optional index lets hot method lookup reuse a precomputed receiver cache. Without it,
    /// the query falls back to direct item-store scans through the same provider.
    pub fn trait_function_candidates_for_receiver(
        &self,
        index: Option<&ItemLookupIndex>,
        receiver_ty: &NominalTy,
        method_name: Option<&str>,
    ) -> Result<Vec<(FunctionRef, TraitApplicability)>, D::Error> {
        let trait_impls = match index {
            Some(index) => index.trait_impls_for_type(receiver_ty.def).to_vec(),
            None => self.target_items.trait_impls_for_type(receiver_ty.def)?,
        };
        self.trait_function_candidates_from_impls(index, trait_impls, receiver_ty, method_name)
    }

    /// Expands already-collected trait impl candidates into trait function candidates.
    ///
    /// The caller owns visibility and overlay rules by deciding which trait impl refs to pass in;
    /// this method owns only receiver applicability and trait-associated function expansion.
    pub fn trait_function_candidates_from_impls(
        &self,
        index: Option<&ItemLookupIndex>,
        trait_impls: Vec<TraitImplRef>,
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
            if let (Some(index), Some(method_name)) = (index, method_name)
                && let Some(functions) =
                    index.trait_functions_by_name(trait_impl.trait_ref, method_name)
            {
                if functions.is_empty() {
                    continue;
                }
                indexed_trait_functions = Some(functions.to_vec());
            }

            let Some(trait_impl_match) = self.trait_impl_match(trait_impl, receiver_ty)? else {
                continue;
            };
            let (applicability, _) = trait_impl_match.into_parts();

            let trait_functions = if let Some(functions) = indexed_trait_functions {
                functions
            } else {
                let trait_functions = if let Some(index) = index
                    && let Some(functions) = index.trait_functions(trait_impl.trait_ref)
                {
                    functions.to_vec()
                } else {
                    item_query
                        .trait_data(trait_impl.trait_ref)?
                        .map(|t| t.functions().collect())
                        .unwrap_or_default()
                };

                // The direct item-store fallback cannot skip the impl check up front, but it can
                // still avoid returning unrelated trait functions to the later method-call filter.
                if let Some(method_name) = method_name {
                    let mut retained = Vec::new();
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

    /// Performs the boolean self-type argument check used by inherent impl methods.
    ///
    /// Type parameters in the impl self type act as wildcards, but concrete arguments must match:
    /// `impl Wrapper<User>` applies to `Wrapper<User>`, not `Wrapper<Project>`.
    fn impl_self_args_match_receiver(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        receiver_ty: &NominalTy,
    ) -> Result<bool, D::Error> {
        // Type parameters in the impl self type act as wildcards. Concrete args such as
        // `impl Wrapper<User>` must equal the receiver's known args.
        let TypeRef::Path(self_ty) = &impl_data.self_ty else {
            return Ok(true);
        };
        let Some(segment) = self_ty.segments.last() else {
            return Ok(true);
        };

        let Some(impl_type_args) = Self::item_tree_type_args(&segment.args) else {
            return Ok(false);
        };
        let Some(receiver_type_args) = Self::ty_args(&receiver_ty.args) else {
            return Ok(false);
        };
        if impl_type_args.len() != receiver_type_args.len() {
            return Ok(false);
        }

        let impl_type_params = Self::impl_type_param_names(&impl_data.generics);
        for (impl_arg, receiver_arg) in impl_type_args.into_iter().zip(receiver_type_args) {
            if impl_arg
                .type_param_name()
                .as_deref()
                .is_some_and(|name| impl_type_params.contains(&name))
            {
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
            if impl_arg_ty != receiver_arg {
                return Ok(false);
            }
        }

        Ok(true)
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

    /// Builds substitutions only when the impl self type structurally matches the receiver.
    ///
    /// This intentionally rejects optimistic cases that are acceptable for trait-method UI
    /// candidates. Type adjustments such as `Deref` must not turn an uncertain impl into a real
    /// receiver type.
    fn impl_self_structural_subst(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        receiver_ty: &NominalTy,
    ) -> Result<Option<TypeSubst>, D::Error> {
        if !Self::impl_header_has_only_plain_type_params(impl_data) {
            return Ok(None);
        }

        let TypeRef::Path(self_ty) = &impl_data.self_ty else {
            return Ok(None);
        };
        let Some(segment) = self_ty.segments.last() else {
            return Ok(None);
        };
        if segment.args.len() != receiver_ty.args.len() {
            return Ok(None);
        }

        let impl_type_params = Self::impl_type_param_names(&impl_data.generics);
        let mut subst = TypeSubst::new();

        for (impl_arg, receiver_arg) in segment.args.iter().zip(&receiver_ty.args) {
            let Some(impl_arg) = impl_arg.type_ref() else {
                return Ok(None);
            };
            let Some(receiver_arg) = receiver_arg.as_ty().cloned() else {
                return Ok(None);
            };

            if let Some(name) = impl_arg.type_param_name()
                && impl_type_params.contains(&name.as_str())
            {
                if !Self::push_structural_subst(&mut subst, name, receiver_arg) {
                    return Ok(None);
                }
                continue;
            }

            if impl_arg.mentions_type_param(&impl_type_params) {
                return Ok(None);
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
                return Ok(None);
            }

            if impl_arg_ty != receiver_arg {
                return Ok(None);
            }
        }

        Ok(Some(subst))
    }

    /// Extracts substitutions by aligning impl self type args with receiver type args.
    ///
    /// For `impl<T> Wrapper<T>` matched with `Wrapper<User>`, this returns `T -> User`.
    fn impl_self_subst(
        generics: &GenericParams,
        self_ty: &TypeRef,
        receiver_args: &[GenericArg],
    ) -> TypeSubst {
        let TypeRef::Path(self_ty) = self_ty else {
            return TypeSubst::new();
        };
        let Some(segment) = self_ty.segments.last() else {
            return TypeSubst::new();
        };

        let impl_type_params = Self::impl_type_param_names(generics);
        let receiver_type_args = receiver_args
            .iter()
            .filter_map(|arg| arg.as_ty().cloned())
            .collect::<Vec<_>>();

        segment
            .args
            .iter()
            .filter_map(ItemGenericArg::type_ref)
            .zip(receiver_type_args)
            .filter_map(|(impl_arg, receiver_arg)| {
                let name = impl_arg.type_param_name()?;
                impl_type_params
                    .contains(&name.as_str())
                    .then_some((name, receiver_arg))
            })
            .collect()
    }

    /// Records a strict direct-param substitution, rejecting conflicting repeated params.
    fn push_structural_subst(subst: &mut TypeSubst, name: Name, ty: Ty) -> bool {
        if let Some(existing_ty) = subst.get(name.as_str()) {
            return existing_ty == &ty;
        }

        subst.push(name, ty);
        true
    }

    /// Lists the type parameters declared by an impl header.
    fn impl_type_param_names(generics: &GenericParams) -> Vec<&str> {
        generics
            .types
            .iter()
            .map(|param| param.name.as_str())
            .collect()
    }

    /// Returns item-tree type args only when no lifetime/const/assoc args were written.
    fn item_tree_type_args(args: &[ItemGenericArg]) -> Option<Vec<&TypeRef>> {
        let mut type_args = Vec::new();
        for arg in args {
            type_args.push(arg.type_ref()?);
        }
        Some(type_args)
    }

    /// Returns type args only when no lifetime/const/assoc args were preserved.
    fn ty_args(args: &[GenericArg]) -> Option<Vec<Ty>> {
        let mut type_args = Vec::new();
        for arg in args {
            type_args.push(arg.as_ty().cloned()?);
        }
        Some(type_args)
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

        push_unique(functions, (function, applicability));
    }

    /// Returns whether the impl header has no constraints that require solving.
    fn impl_header_has_only_plain_type_params(impl_data: &ImplData) -> bool {
        impl_data.generics.lifetimes.is_empty()
            && impl_data.generics.consts.is_empty()
            && impl_data.generics.where_predicates.is_empty()
            && impl_data
                .generics
                .types
                .iter()
                .all(|param| param.bounds.is_empty() && param.default.is_none())
            && impl_data
                .trait_ref
                .as_ref()
                .is_none_or(|trait_ref| !trait_ref.has_generic_args())
    }

    /// Returns whether the impl header has no generic or where-clause uncertainty.
    fn impl_header_is_definitely_direct(impl_data: &ImplData) -> bool {
        impl_data.generics.lifetimes.is_empty()
            && impl_data.generics.types.is_empty()
            && impl_data.generics.consts.is_empty()
            && impl_data.generics.where_predicates.is_empty()
            && impl_data
                .trait_ref
                .as_ref()
                .is_none_or(|trait_ref| !trait_ref.has_generic_args())
    }

    /// Returns whether comparing this type as a generic argument would overstate certainty.
    fn type_arg_comparison_is_uncertain(ty: &Ty) -> bool {
        match ty {
            Ty::Syntax(_) | Ty::Unknown => true,
            Ty::Reference { inner, .. } => Self::type_arg_comparison_is_uncertain(inner),
            Ty::Unit | Ty::Never | Ty::Primitive(_) | Ty::Nominal(_) | Ty::SelfTy(_) => false,
        }
    }
}

fn push_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    if !items.contains(&item) {
        items.push(item);
    }
}
