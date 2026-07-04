//! Strict impl matching for type-changing operations.
//!
//! `Deref` and structural inherent lookup affect real type facts. This module therefore rejects
//! uncertain headers instead of returning maybe-applicable matches.

use crate::inference::InferenceTable;
use crate::{
    GenericArg, NominalTy, TraitGoal, TraitSelectionOptions, TraitSelectionQuery, Ty, TypeSubst,
};
use rg_ir_model::hir::items::ImplData;
use rg_ir_model::items::{GenericArg as ItemGenericArg, TypeBound, TypePath, TypeRef};
use rg_ir_model::{
    ImplRef, Mutability, Path, TraitApplicability, TraitImplRef, TypePathResolution,
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_text::Name;

use super::ImplMatcher;

impl<'query, D, I> ImplMatcher<'query, D, I>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
{
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

    /// Matches one trait impl for contexts that perform a real type adjustment.
    ///
    /// This is stricter than method candidate matching: only direct impl type parameters such as
    /// `Wrapper<T>` are bindable. A trailing impl parameter may have bounds only when it
    /// corresponds to an omitted type-definition default, such as `Vec<T, A = Global>` matched by
    /// a receiver written as `Vec<User>`. Nested generic patterns like `Wrapper<Option<T>>`, where
    /// clauses, other bounded params, lifetimes, and const generics are rejected until a real solver
    /// exists.
    pub fn trait_impl_structural_match(
        &self,
        trait_impl: TraitImplRef,
        receiver_ty: &NominalTy,
    ) -> Result<Option<TypeSubst>, D::Error> {
        let item_query = self.item_paths.items();
        let Some(impl_data) = item_query.impl_data(trait_impl.impl_ref)? else {
            return Ok(None);
        };
        if !impl_data.resolved_self_ty.is(&receiver_ty.def)
            || !impl_data.resolved_trait_ref.is(&trait_impl.trait_ref)
        {
            return Ok(None);
        }

        self.impl_self_structural_subst(trait_impl.impl_ref, impl_data, receiver_ty)
    }

    /// Builds substitutions only when the impl self type structurally matches the receiver.
    ///
    /// This intentionally rejects optimistic cases that are acceptable for trait-method UI
    /// candidates. Type adjustments such as `Deref` must not turn an uncertain impl into a real
    /// receiver type. The only extra shape accepted here is a trailing type-definition default,
    /// because `Vec<User>` and `Vec<User, Global>` are the same concrete receiver for matching an
    /// impl header written as `Vec<T, A>`.
    fn impl_self_structural_subst(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        receiver_ty: &NominalTy,
    ) -> Result<Option<TypeSubst>, D::Error> {
        let TypeRef::Path(self_ty) = &impl_data.self_ty else {
            return Ok(None);
        };
        let Some(segment) = self_ty.segments.last() else {
            return Ok(None);
        };
        let Some(defaulted_missing_params) = self.defaulted_missing_impl_type_params(
            impl_data,
            receiver_ty,
            segment.args.as_slice(),
        )?
        else {
            return Ok(None);
        };
        if !Self::impl_header_is_structural_with_defaulted_tail(
            impl_data,
            &defaulted_missing_params,
        ) {
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

        if !defaulted_missing_params.is_empty() {
            let item_query = self.item_paths.items();
            let Some(type_def_owner) = item_query.type_def_owner(receiver_ty.def)? else {
                return Ok(None);
            };
            let default_context = TypePathContext {
                module: type_def_owner,
                impl_ref: None,
            };

            // A receiver like `Vec<User>` has already chosen the type definition's trailing
            // defaults. Bind the corresponding impl params so later associated-type projection
            // sees the same complete `Self` type as Rust does. If that omitted impl param had
            // bounds, prove them after substitution; otherwise this strict adjustment path would
            // accept exactly the kind of uncertain impl it is meant to reject.
            for param in defaulted_missing_params {
                let param_name = param.name.clone();
                let default_ty = self.item_paths.resolve_type_ref(
                    &param.default,
                    default_context,
                    Ty::syntax(param.default.clone()),
                    &subst,
                )?;
                if Self::type_arg_comparison_is_uncertain(&default_ty)
                    || !Self::push_structural_subst(&mut subst, param_name.clone(), default_ty)
                {
                    return Ok(None);
                }
                if !self.defaulted_param_bounds_hold(impl_ref, impl_data, &param_name, &subst)? {
                    return Ok(None);
                }
            }
        }

        Ok(Some(subst))
    }

    fn impl_header_is_structural_with_defaulted_tail(
        impl_data: &ImplData,
        defaulted_missing_params: &[DefaultedMissingImplTypeParam],
    ) -> bool {
        impl_data.generics.lifetimes.is_empty()
            && impl_data.generics.consts.is_empty()
            && impl_data.generics.where_predicates.is_empty()
            && impl_data.generics.types.iter().all(|param| {
                param.default.is_none()
                    && (param.bounds.is_empty()
                        || defaulted_missing_params
                            .iter()
                            .any(|missing| missing.name.as_str() == param.name.as_str()))
            })
            && impl_data
                .trait_ref
                .as_ref()
                .is_some_and(|trait_ref| !trait_ref.has_generic_args())
    }

    fn defaulted_missing_impl_type_params(
        &self,
        impl_data: &ImplData,
        receiver_ty: &NominalTy,
        impl_args: &[ItemGenericArg],
    ) -> Result<Option<Vec<DefaultedMissingImplTypeParam>>, D::Error> {
        let receiver_args = Self::ty_args(&receiver_ty.args);
        let Some(receiver_args) = receiver_args else {
            return Ok(None);
        };
        if impl_args.len() < receiver_args.len() {
            return Ok(None);
        }
        let Some(impl_type_args) = Self::item_tree_type_args(impl_args) else {
            return Ok(None);
        };
        if impl_type_args.len() == receiver_args.len() {
            return Ok(Some(Vec::new()));
        }

        let item_query = self.item_paths.items();
        let Some(type_def_generics) = item_query.generic_params_for_type_def(receiver_ty.def)?
        else {
            return Ok(None);
        };
        if type_def_generics.types.len() < impl_type_args.len() {
            return Ok(None);
        }

        let impl_type_params = Self::impl_type_param_names(&impl_data.generics);
        let mut missing_params = Vec::new();
        for (idx, impl_arg) in impl_type_args.iter().enumerate().skip(receiver_args.len()) {
            let Some(param_name) = impl_arg.type_param_name() else {
                return Ok(None);
            };
            if !impl_type_params.contains(&param_name.as_str()) {
                return Ok(None);
            }
            let Some(default) = type_def_generics.types[idx].default.clone() else {
                return Ok(None);
            };
            missing_params.push(DefaultedMissingImplTypeParam {
                name: param_name,
                default,
            });
        }

        Ok(Some(missing_params))
    }

    fn defaulted_param_bounds_hold(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        param_name: &Name,
        subst: &TypeSubst,
    ) -> Result<bool, D::Error> {
        let Some(default_ty) = subst.get(param_name.as_str()) else {
            return Ok(false);
        };
        let Some(param_data) = impl_data
            .generics
            .types
            .iter()
            .find(|param| param.name.as_str() == param_name.as_str())
        else {
            return Ok(false);
        };

        for bound in &param_data.bounds {
            if !self.default_ty_satisfies_bound(impl_ref, impl_data, default_ty, subst, bound)? {
                return Ok(false);
            }
        }

        Ok(true)
    }

    fn default_ty_satisfies_bound(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        default_ty: &Ty,
        subst: &TypeSubst,
        bound: &TypeBound,
    ) -> Result<bool, D::Error> {
        let TypeBound::Trait(TypeRef::Path(bound_path)) = bound else {
            return Ok(false);
        };
        let context = TypePathContext {
            module: impl_data.owner,
            impl_ref: Some(impl_ref),
        };
        let TypePathResolution::Trait(trait_ref) = self
            .item_paths
            .resolve_type_path(context, &Path::from_type_path(bound_path))?
        else {
            return Ok(false);
        };
        let Some(args) = self.infer_generic_args_from_bound_path(bound_path, context, subst)?
        else {
            return Ok(false);
        };

        let goal = TraitGoal {
            self_ty: default_ty.clone(),
            trait_ref,
            args,
        };
        let table = InferenceTable::new();
        let mut definite_matches = 0usize;
        for trait_impl in self.target_items.trait_impls_for_trait(trait_ref)? {
            let Some(selection) = TraitSelectionQuery::probe_visible_trait_impl(
                &self.item_paths,
                &self.target_items,
                &goal,
                &table,
                trait_impl,
                TraitSelectionOptions::new(),
            )?
            else {
                continue;
            };
            if selection.applicability != TraitApplicability::Yes {
                continue;
            }
            definite_matches += 1;
            if definite_matches > 1 {
                return Ok(false);
            }
        }

        Ok(definite_matches == 1)
    }

    fn infer_generic_args_from_bound_path(
        &self,
        path: &TypePath,
        context: TypePathContext,
        subst: &TypeSubst,
    ) -> Result<Option<Vec<GenericArg>>, D::Error> {
        let Some(segment) = path.segments.last() else {
            return Ok(Some(Vec::new()));
        };

        let mut args = Vec::new();
        for arg in &segment.args {
            let Some(arg) = self.infer_generic_arg_from_bound_arg(arg, context, subst)? else {
                return Ok(None);
            };
            args.push(arg);
        }
        Ok(Some(args))
    }

    fn infer_generic_arg_from_bound_arg(
        &self,
        arg: &ItemGenericArg,
        context: TypePathContext,
        subst: &TypeSubst,
    ) -> Result<Option<GenericArg>, D::Error> {
        Ok(Some(match arg {
            ItemGenericArg::Type(ty) => {
                let Some(ty) = self.infer_ty_from_bound_type_ref(ty, context, subst)? else {
                    return Ok(None);
                };
                GenericArg::Type(Box::new(ty))
            }
            ItemGenericArg::Lifetime(lifetime) => GenericArg::Lifetime(lifetime.clone()),
            ItemGenericArg::Const(value) => GenericArg::Const(value.clone()),
            ItemGenericArg::FnTraitArgs { params, ret } => {
                let mut projected_params = Vec::new();
                for param in params {
                    let Some(param) = self.infer_ty_from_bound_type_ref(param, context, subst)?
                    else {
                        return Ok(None);
                    };
                    projected_params.push(param);
                }
                let Some(ret) = self.infer_ty_from_bound_type_ref(ret, context, subst)? else {
                    return Ok(None);
                };
                GenericArg::FnTraitArgs {
                    params: projected_params,
                    ret: Box::new(ret),
                }
            }
            ItemGenericArg::AssocType { name, ty } => {
                let ty = match ty {
                    Some(ty) => {
                        let Some(ty) = self.infer_ty_from_bound_type_ref(ty, context, subst)?
                        else {
                            return Ok(None);
                        };
                        Some(Box::new(ty))
                    }
                    None => None,
                };
                GenericArg::AssocType {
                    name: name.clone(),
                    ty,
                }
            }
            ItemGenericArg::Unsupported(_) => return Ok(None),
        }))
    }

    fn infer_ty_from_bound_type_ref(
        &self,
        ty: &TypeRef,
        context: TypePathContext,
        subst: &TypeSubst,
    ) -> Result<Option<Ty>, D::Error> {
        let resolved_ty =
            self.item_paths
                .resolve_type_ref(ty, context, Ty::syntax(ty.clone()), subst)?;
        Ok((!Self::type_arg_comparison_is_uncertain(&resolved_ty)).then_some(resolved_ty))
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
        receiver_mutability: Mutability,
    ) -> bool {
        impl_mutability == receiver_mutability
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

    /// Records a strict direct-param substitution, rejecting conflicting repeated params.
    fn push_structural_subst(subst: &mut TypeSubst, name: Name, ty: Ty) -> bool {
        if let Some(existing_ty) = subst.get(name.as_str()) {
            return existing_ty == &ty;
        }

        subst.push(name, ty);
        true
    }
}

struct DefaultedMissingImplTypeParam {
    name: Name,
    default: TypeRef,
}
