//! Strict impl matching for type-changing operations.
//!
//! `Deref`, structural inherent lookup, and associated-type projection all affect real type facts.
//! This module therefore rejects uncertain headers instead of returning maybe-applicable matches.

use crate::{GenericArg, NominalTy, Ty, TypeSubst};
use rg_ir_model::hir::items::ImplData;
use rg_ir_model::items::{GenericArg as ItemGenericArg, TypePath, TypeRef};
use rg_ir_model::{ImplRef, Mutability, Path, TraitImplRef, TypePathResolution};
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
        if !impl_data.resolved_self_ty.is(&receiver_ty.def)
            || !impl_data.resolved_trait_ref.is(&trait_impl.trait_ref)
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
        if !impl_data.resolved_trait_ref.is(&trait_impl.trait_ref)
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
            TypePathResolution::TypeDef(type_def) | TypePathResolution::SelfType(type_def) => {
                for nominal in receiver_ty.as_nominals() {
                    if nominal.def != type_def {
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
            TypePathResolution::TypeAlias(_)
            | TypePathResolution::Trait(_)
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
        receiver_mutability: Mutability,
    ) -> bool {
        impl_mutability == receiver_mutability
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
}

struct ImplParamNames<'a> {
    types: &'a [&'a str],
    lifetimes: &'a [&'a str],
    consts: &'a [&'a str],
}
