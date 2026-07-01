//! Shallow impl matching for receiver-based item queries.
//!
//! `ImplMatcher` is the stable façade used by member lookup, adjustments, and body queries. The
//! implementation is split by policy, because those callers intentionally have different tolerance
//! for uncertainty: editor candidates may keep `Maybe` impls, while real type adjustments must
//! reject anything that would require a solver.

mod inherent;
mod projection;
mod trait_methods;

use crate::{GenericArg, ItemPathQuery, NominalTy, Ty, TypeSubst};
use rg_ir_model::TraitApplicability;
use rg_ir_model::hir::items::ImplData;
use rg_ir_model::items::{GenericArg as ItemGenericArg, GenericParams, TypeRef};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TargetItemQuery};

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

    /// Builds receiver substitutions from already-loaded impl data.
    pub fn impl_self_subst_for_impl(
        &self,
        impl_data: &ImplData,
        receiver_ty: &NominalTy,
    ) -> TypeSubst {
        Self::impl_self_subst(&impl_data.generics, &impl_data.self_ty, &receiver_ty.args)
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

    /// Classifies direct headers as proven and generic/conditional headers as maybe-applicable.
    fn impl_header_applicability(impl_data: &ImplData) -> TraitApplicability {
        if Self::impl_header_is_definitely_direct(impl_data) {
            TraitApplicability::Yes
        } else {
            TraitApplicability::Maybe
        }
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
            Ty::Unit
            | Ty::Never
            | Ty::Primitive(_)
            | Ty::Closure(_)
            | Ty::Nominal(_)
            | Ty::SelfTy(_) => false,
        }
    }
}
