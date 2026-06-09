//! Shallow impl matching for receiver-based item queries.
//!
//! The matchers own the small amount of generic reasoning used by method lookup and associated
//! items. They compare explicit impl self types against known receiver types and produce the
//! substitutions that make associated signatures readable in the receiver context.

use crate::{GenericArg, ItemPathQuery, NominalTy, RefMutability, Ty, TypeSubst};
use rg_ir_model::items::{
    GenericArg as ItemGenericArg, GenericParams, Mutability, TypePath, TypeRef,
};
use rg_ir_model::{
    FunctionRef, ImplRef, ItemOwner, Path, TraitApplicability, TraitImplRef, TypePathResolution,
    hir::items::ImplData,
};
use rg_ir_storage::{
    DefMapSource, ItemLookupIndex, ItemStoreSource, TargetItemQuery, TypePathContext,
};
use rg_std::UniqueVec;
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

    /// Matches an inherent impl whose `Self` type is structural rather than nominal.
    ///
    /// This covers impl headers such as `impl<T> [T]`, which cannot participate in the
    /// `TypeDefRef`-keyed receiver index used for nominal impls. The match is deliberately strict:
    /// only already-modeled structural types and direct type-parameter substitutions are accepted.
    pub fn structural_inherent_impl_subst(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        receiver_ty: &Ty,
    ) -> Result<Option<TypeSubst>, D::Error> {
        // Structural impl lookup is a precise receiver adjustment, not an optimistic completion
        // heuristic. Once generic constraints appear, a real solver would be needed to know
        // whether the impl applies.
        if impl_data.trait_ref.is_some()
            || !Self::impl_header_has_only_plain_type_params(impl_data)
            || !Self::type_ref_uses_structural_receiver_lookup(&impl_data.self_ty)
        {
            return Ok(None);
        }

        let impl_type_params = Self::impl_type_param_names(&impl_data.generics);
        let mut subst = TypeSubst::new();
        if self.structural_type_ref_matches_ty(
            impl_ref,
            impl_data,
            &impl_data.self_ty,
            receiver_ty,
            &impl_type_params,
            &mut subst,
        )? {
            Ok(Some(subst))
        } else {
            Ok(None)
        }
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

    /// Matches a trait impl for associated-type projection against any known receiver `Ty`.
    ///
    /// This is the strict path used by adjustments and iterator item flow. It accepts direct
    /// generic bindings inside already-modeled nominal or structural self types, but rejects
    /// bounded blanket impls unless the caller handles a specific blanket shape itself.
    pub(crate) fn trait_impl_projection_subst_for_ty(
        &self,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        receiver_ty: &Ty,
    ) -> Result<Option<TypeSubst>, D::Error> {
        if !impl_data
            .resolved_trait_refs
            .contains(&trait_impl.trait_ref)
            || !Self::impl_header_is_projectable(impl_data)
            || impl_data.self_ty.type_param_name().is_some_and(|name| {
                Self::impl_type_param_names(&impl_data.generics).contains(&name.as_str())
            })
        {
            return Ok(None);
        }

        let impl_type_params = Self::impl_type_param_names(&impl_data.generics);
        let impl_lifetime_params = Self::impl_lifetime_param_names(&impl_data.generics);
        let impl_const_params = Self::impl_const_param_names(&impl_data.generics);
        let mut subst = TypeSubst::new();

        if self.projection_type_ref_matches_ty(
            trait_impl.impl_ref,
            impl_data,
            &impl_data.self_ty,
            receiver_ty,
            &ImplParamNames {
                types: &impl_type_params,
                lifetimes: &impl_lifetime_params,
                consts: &impl_const_params,
            },
            &mut subst,
        )? {
            Ok(Some(subst))
        } else {
            Ok(None)
        }
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
            Some(index) => index
                .trait_impls_for_type(receiver_ty.def)
                .cloned()
                .unwrap_or_default(),
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
            if let (Some(index), Some(method_name)) = (index, method_name)
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
            } else {
                let trait_functions = if let Some(index) = index
                    && let Some(functions) = index.trait_functions(trait_impl.trait_ref)
                {
                    functions.clone()
                } else {
                    item_query
                        .trait_data(trait_impl.trait_ref)?
                        .map(|t| t.functions().collect())
                        .unwrap_or_default()
                };

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
        // Type and const parameters in the impl self type act as wildcards. Concrete args such as
        // `impl Wrapper<User>` or `impl Foo<1>` must equal the receiver's known args. Lifetimes do
        // not select inherent impls, so they only need to line up as lifetime arguments.
        let TypeRef::Path(self_ty) = &impl_data.self_ty else {
            return Ok(true);
        };
        let Some(segment) = self_ty.segments.last() else {
            return Ok(true);
        };

        if segment.args.len() != receiver_ty.args.len() {
            return Ok(false);
        }

        let impl_type_params = Self::impl_type_param_names(&impl_data.generics);
        let impl_const_params = Self::impl_const_param_names(&impl_data.generics);
        for (impl_arg, receiver_arg) in segment.args.iter().zip(&receiver_ty.args) {
            if !self.impl_self_arg_matches_receiver(
                impl_ref,
                impl_data,
                impl_arg,
                receiver_arg,
                &impl_type_params,
                &impl_const_params,
            )? {
                return Ok(false);
            }
        }

        Ok(true)
    }

    fn impl_self_arg_matches_receiver(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        impl_arg: &ItemGenericArg,
        receiver_arg: &GenericArg,
        impl_type_params: &[&str],
        impl_const_params: &[&str],
    ) -> Result<bool, D::Error> {
        match impl_arg {
            ItemGenericArg::Type(impl_arg) => {
                let Some(receiver_arg) = receiver_arg.as_ty().cloned() else {
                    return Ok(false);
                };
                if impl_arg
                    .type_param_name()
                    .as_deref()
                    .is_some_and(|name| impl_type_params.contains(&name))
                {
                    return Ok(true);
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
                Ok(impl_arg_ty == receiver_arg)
            }
            ItemGenericArg::Lifetime(_) => Ok(matches!(receiver_arg, GenericArg::Lifetime(_))),
            ItemGenericArg::Const(impl_arg) => {
                let GenericArg::Const(receiver_arg) = receiver_arg else {
                    return Ok(false);
                };
                Ok(impl_const_params.contains(&impl_arg.as_str()) || impl_arg == receiver_arg)
            }
            ItemGenericArg::FnTraitArgs { .. }
            | ItemGenericArg::AssocType { .. }
            | ItemGenericArg::Unsupported(_) => Ok(false),
        }
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

    /// Recursively matches an impl `Self` type against a receiver for real type projection.
    fn projection_type_ref_matches_ty(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        impl_ty: &TypeRef,
        receiver_ty: &Ty,
        params: &ImplParamNames<'_>,
        subst: &mut TypeSubst,
    ) -> Result<bool, D::Error> {
        // A direct type parameter inside a concrete shape is the only binding operation here.
        // Bare blanket impls such as `impl<T> Trait for T` are rejected before this matcher is
        // entered because they need trait-bound reasoning.
        if let Some(name) = impl_ty.type_param_name()
            && params.types.contains(&name.as_str())
        {
            return Ok(Self::push_projection_subst(
                subst,
                name,
                receiver_ty.clone(),
            ));
        }

        Ok(match (impl_ty, receiver_ty) {
            (TypeRef::Unit, Ty::Unit) | (TypeRef::Never, Ty::Never) => true,
            (TypeRef::Tuple(impl_fields), Ty::Tuple(receiver_fields)) => {
                if impl_fields.len() != receiver_fields.len() {
                    return Ok(false);
                }
                for (impl_field, receiver_field) in impl_fields.iter().zip(receiver_fields) {
                    if !self.projection_type_ref_matches_ty(
                        impl_ref,
                        impl_data,
                        impl_field,
                        receiver_field,
                        params,
                        subst,
                    )? {
                        return Ok(false);
                    }
                }
                true
            }
            (
                TypeRef::Reference {
                    mutability: impl_mutability,
                    inner: impl_inner,
                    ..
                },
                Ty::Reference {
                    mutability: receiver_mutability,
                    inner: receiver_inner,
                },
            ) if Self::ref_mutability_matches(*impl_mutability, *receiver_mutability) => self
                .projection_type_ref_matches_ty(
                    impl_ref,
                    impl_data,
                    impl_inner,
                    receiver_inner,
                    params,
                    subst,
                )?,
            (TypeRef::Slice(impl_inner), Ty::Slice(receiver_inner)) => self
                .projection_type_ref_matches_ty(
                    impl_ref,
                    impl_data,
                    impl_inner,
                    receiver_inner,
                    params,
                    subst,
                )?,
            (
                TypeRef::Array {
                    inner: impl_inner,
                    len: impl_len,
                },
                Ty::Array {
                    inner: receiver_inner,
                    len: receiver_len,
                },
            ) if Self::array_len_matches(impl_len, receiver_len, params.consts) => self
                .projection_type_ref_matches_ty(
                    impl_ref,
                    impl_data,
                    impl_inner,
                    receiver_inner,
                    params,
                    subst,
                )?,
            (TypeRef::Path(path), _) => self.projection_path_type_ref_matches_ty(
                impl_ref,
                impl_data,
                path,
                receiver_ty,
                params,
                subst,
            )?,
            _ => {
                if impl_ty.mentions_type_param(params.types) {
                    return Ok(false);
                }

                let context = TypePathContext {
                    module: impl_data.owner,
                    impl_ref: Some(impl_ref),
                };
                let impl_ty = self.item_paths.resolve_type_ref(
                    impl_ty,
                    context,
                    Ty::syntax(impl_ty.clone()),
                    &TypeSubst::new(),
                )?;
                impl_ty.is_projectable() && receiver_ty.is_projectable() && impl_ty == *receiver_ty
            }
        })
    }

    fn projection_path_type_ref_matches_ty(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        impl_path: &TypePath,
        receiver_ty: &Ty,
        params: &ImplParamNames<'_>,
        subst: &mut TypeSubst,
    ) -> Result<bool, D::Error> {
        let path = Path::from_type_path(impl_path);
        let context = TypePathContext {
            module: impl_data.owner,
            impl_ref: Some(impl_ref),
        };
        let impl_args = impl_path
            .segments
            .last()
            .map(|segment| segment.args.as_slice())
            .unwrap_or(&[]);

        match self.item_paths.resolve_type_path(context, &path)? {
            TypePathResolution::TypeDefs(type_defs) | TypePathResolution::SelfType(type_defs) => {
                for nominal in receiver_ty.as_nominals() {
                    if !type_defs.contains(&nominal.def) {
                        continue;
                    }
                    if self.projection_generic_args_match_ty_args(
                        impl_ref,
                        impl_data,
                        impl_args,
                        &nominal.args,
                        params,
                        subst,
                    )? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            TypePathResolution::TypeAliases(_)
            | TypePathResolution::Traits(_)
            | TypePathResolution::Unknown => {
                if let Some(name) = impl_path.single_name()
                    && params.types.contains(&name.as_str())
                {
                    return Ok(Self::push_projection_subst(
                        subst,
                        name.clone(),
                        receiver_ty.clone(),
                    ));
                }

                let impl_ty = self.item_paths.resolve_type_ref(
                    &TypeRef::Path(impl_path.clone()),
                    context,
                    Ty::syntax(TypeRef::Path(impl_path.clone())),
                    &TypeSubst::new(),
                )?;
                Ok(impl_ty.is_projectable()
                    && receiver_ty.is_projectable()
                    && impl_ty == *receiver_ty)
            }
        }
    }

    fn projection_generic_args_match_ty_args(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        impl_args: &[ItemGenericArg],
        receiver_args: &[GenericArg],
        params: &ImplParamNames<'_>,
        subst: &mut TypeSubst,
    ) -> Result<bool, D::Error> {
        if impl_args.len() != receiver_args.len() {
            return Ok(false);
        }

        for (impl_arg, receiver_arg) in impl_args.iter().zip(receiver_args) {
            match impl_arg {
                ItemGenericArg::Type(impl_ty) => {
                    let Some(receiver_ty) = receiver_arg.as_ty() else {
                        return Ok(false);
                    };
                    if !self.projection_type_ref_matches_ty(
                        impl_ref,
                        impl_data,
                        impl_ty,
                        receiver_ty,
                        params,
                        subst,
                    )? {
                        return Ok(false);
                    }
                }
                ItemGenericArg::Lifetime(lifetime) => {
                    if !matches!(receiver_arg, GenericArg::Lifetime(_))
                        || (!params.lifetimes.contains(&lifetime.as_str())
                            && !matches!(receiver_arg, GenericArg::Lifetime(receiver) if receiver == lifetime))
                    {
                        return Ok(false);
                    }
                }
                ItemGenericArg::Const(value) => {
                    if params.consts.contains(&value.as_str()) {
                        continue;
                    }
                    if !matches!(receiver_arg, GenericArg::Const(receiver) if receiver == value) {
                        return Ok(false);
                    }
                }
                ItemGenericArg::FnTraitArgs { .. }
                | ItemGenericArg::AssocType { .. }
                | ItemGenericArg::Unsupported(_) => return Ok(false),
            }
        }

        Ok(true)
    }

    /// Recursively matches a structural impl `Self` type against an adjusted receiver type.
    fn structural_type_ref_matches_ty(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        impl_ty: &TypeRef,
        receiver_ty: &Ty,
        impl_type_params: &[&str],
        subst: &mut TypeSubst,
    ) -> Result<bool, D::Error> {
        // A bare impl type param is the only unification-like operation this matcher performs:
        // `impl<T> [T]` matched with `[Package]` records `T -> Package`.
        if let Some(name) = impl_ty.type_param_name()
            && impl_type_params.contains(&name.as_str())
        {
            return Ok(Self::push_structural_subst(
                subst,
                name,
                receiver_ty.clone(),
            ));
        }

        Ok(match (impl_ty, receiver_ty) {
            (TypeRef::Tuple(impl_fields), Ty::Tuple(receiver_fields)) => {
                if impl_fields.len() != receiver_fields.len() {
                    return Ok(false);
                }

                for (impl_field, receiver_field) in impl_fields.iter().zip(receiver_fields) {
                    if !self.structural_type_ref_matches_ty(
                        impl_ref,
                        impl_data,
                        impl_field,
                        receiver_field,
                        impl_type_params,
                        subst,
                    )? {
                        return Ok(false);
                    }
                }
                true
            }
            (TypeRef::Slice(impl_inner), Ty::Slice(receiver_inner)) => self
                .structural_type_ref_matches_ty(
                    impl_ref,
                    impl_data,
                    impl_inner,
                    receiver_inner,
                    impl_type_params,
                    subst,
                )?,
            (
                TypeRef::Array {
                    inner: impl_inner,
                    len: impl_len,
                },
                Ty::Array {
                    inner: receiver_inner,
                    len: receiver_len,
                },
            ) if impl_len == receiver_len => self.structural_type_ref_matches_ty(
                impl_ref,
                impl_data,
                impl_inner,
                receiver_inner,
                impl_type_params,
                subst,
            )?,
            (
                TypeRef::Reference {
                    mutability: impl_mutability,
                    inner: impl_inner,
                    ..
                },
                Ty::Reference {
                    mutability: receiver_mutability,
                    inner: receiver_inner,
                },
            ) if Self::ref_mutability_matches(*impl_mutability, *receiver_mutability) => self
                .structural_type_ref_matches_ty(
                    impl_ref,
                    impl_data,
                    impl_inner,
                    receiver_inner,
                    impl_type_params,
                    subst,
                )?,
            _ => {
                // If a structural shape contains a nested generic pattern we do not understand,
                // reject it instead of guessing. Concrete nested types can still be resolved and
                // compared directly below.
                if impl_ty.mentions_type_param(impl_type_params) {
                    return Ok(false);
                }

                let context = TypePathContext {
                    module: impl_data.owner,
                    impl_ref: Some(impl_ref),
                };
                let impl_ty = self.item_paths.resolve_type_ref(
                    impl_ty,
                    context,
                    Ty::syntax(impl_ty.clone()),
                    &TypeSubst::new(),
                )?;
                !Self::type_arg_comparison_is_uncertain(&impl_ty)
                    && !Self::type_arg_comparison_is_uncertain(receiver_ty)
                    && impl_ty == *receiver_ty
            }
        })
    }

    fn ref_mutability_matches(
        impl_mutability: Mutability,
        receiver_mutability: RefMutability,
    ) -> bool {
        matches!(
            (impl_mutability, receiver_mutability),
            (Mutability::Shared, RefMutability::Shared)
                | (Mutability::Mutable, RefMutability::Mutable)
        )
    }

    fn array_len_matches(
        impl_len: &Option<String>,
        receiver_len: &Option<String>,
        const_params: &[&str],
    ) -> bool {
        match impl_len {
            Some(len) if const_params.contains(&len.as_str()) => true,
            _ => impl_len == receiver_len,
        }
    }

    fn type_ref_uses_structural_receiver_lookup(ty: &TypeRef) -> bool {
        matches!(
            ty,
            TypeRef::Tuple(_)
                | TypeRef::Reference { .. }
                | TypeRef::Slice(_)
                | TypeRef::Array { .. }
        )
    }

    /// Records a strict projection substitution, rejecting unknowns and repeated conflicts.
    fn push_projection_subst(subst: &mut TypeSubst, name: Name, ty: Ty) -> bool {
        if !ty.is_projectable() {
            return false;
        }

        if let Some(existing_ty) = subst.get(name.as_str()) {
            return existing_ty == &ty;
        }

        subst.push(name, ty);
        true
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

    /// Lists the lifetime parameters declared by an impl header.
    fn impl_lifetime_param_names(generics: &GenericParams) -> Vec<&str> {
        generics
            .lifetimes
            .iter()
            .map(|param| param.name.as_str())
            .collect()
    }

    /// Lists the const parameters declared by an impl header.
    fn impl_const_param_names(generics: &GenericParams) -> Vec<&str> {
        generics
            .consts
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

        functions.push((function, applicability));
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

    /// Returns whether an impl is simple enough for real associated-type projection.
    fn impl_header_is_projectable(impl_data: &ImplData) -> bool {
        impl_data.generics.where_predicates.is_empty()
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
            && impl_data
                .generics
                .consts
                .iter()
                .all(|param| param.default.is_none())
            && impl_data
                .trait_ref
                .as_ref()
                .is_some_and(|trait_ref| !trait_ref.has_generic_args())
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
            Ty::Tuple(fields) => fields.iter().any(Self::type_arg_comparison_is_uncertain),
            Ty::Array { inner, .. } | Ty::Slice(inner) => {
                Self::type_arg_comparison_is_uncertain(inner)
            }
            Ty::Reference { inner, .. } => Self::type_arg_comparison_is_uncertain(inner),
            Ty::Opaque { .. } => true,
            Ty::Unit | Ty::Never | Ty::Primitive(_) | Ty::Nominal(_) | Ty::SelfTy(_) => false,
        }
    }
}

struct ImplParamNames<'a> {
    types: &'a [&'a str],
    lifetimes: &'a [&'a str],
    consts: &'a [&'a str],
}
