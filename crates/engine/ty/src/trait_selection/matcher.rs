use rg_ir_model::hir::items::ImplData;
use rg_ir_model::items::{GenericArg as ItemGenericArg, GenericParams, TypeBound, TypeRef};
use rg_ir_model::{TraitApplicability, TraitImplRef, TypeDefRef};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_text::Name;

use super::TraitGoal;
use crate::ItemPathQuery;
use crate::inference::{
    InferGenericArg, InferTy, InferTypeSubst, InferenceConflict, InferenceTable,
};
use crate::{Ty, TypeSubst};

pub(super) struct CandidateMatcher<'matcher, 'query, D, I> {
    item_paths: &'matcher ItemPathQuery<'query, D, I>,
}

impl<'matcher, 'query, D, I> CandidateMatcher<'matcher, 'query, D, I>
where
    D: DefMapSource<Error = I::Error>,
    I: ItemStoreSource<'query>,
{
    pub(super) fn new(item_paths: &'matcher ItemPathQuery<'query, D, I>) -> Self {
        Self { item_paths }
    }

    pub(super) fn match_goal(
        &self,
        goal: &TraitGoal,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        table: &mut InferenceTable,
        subst: &mut InferTypeSubst,
    ) -> Result<Option<TraitApplicability>, I::Error> {
        let Some(self_applicability) =
            self.match_self_ty(goal, trait_impl, impl_data, table, subst)?
        else {
            return Ok(None);
        };

        let Some(trait_applicability) =
            self.match_trait_args(goal, trait_impl, impl_data, table, subst)?
        else {
            return Ok(None);
        };

        Ok(Some(self_applicability.and(trait_applicability)))
    }

    fn match_self_ty(
        &self,
        goal: &TraitGoal,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        table: &mut InferenceTable,
        subst: &mut InferTypeSubst,
    ) -> Result<Option<TraitApplicability>, I::Error> {
        if let Some(self_def) = impl_data.resolved_self_ty.as_option() {
            return self
                .match_nominal_self_ty(goal, trait_impl, *self_def, impl_data, table, subst);
        }

        // Bare blanket impls such as `impl<T> Trait for T` need recursive trait reasoning once
        // bounds enter the picture. The first slice deliberately leaves them to later work.
        if impl_data
            .self_ty
            .type_param_name()
            .is_some_and(|name| Self::is_impl_type_param(&impl_data.generics, &name))
        {
            return Ok(None);
        }

        self.match_type_ref(
            trait_impl,
            impl_data,
            &impl_data.self_ty,
            &goal.self_ty,
            table,
            subst,
        )
    }

    fn match_nominal_self_ty(
        &self,
        goal: &TraitGoal,
        trait_impl: TraitImplRef,
        self_def: TypeDefRef,
        impl_data: &ImplData,
        table: &mut InferenceTable,
        subst: &mut InferTypeSubst,
    ) -> Result<Option<TraitApplicability>, I::Error> {
        let self_ty = table.resolve_root_var(&goal.self_ty);
        let (InferTy::Nominal(goal_ty) | InferTy::SelfTy(goal_ty)) = &self_ty else {
            return Ok(Self::unknown_self_applicability(&self_ty));
        };
        if goal_ty.def != self_def {
            return Ok(None);
        }

        let TypeRef::Path(self_path) = &impl_data.self_ty else {
            return Ok(Some(TraitApplicability::Maybe));
        };
        let Some(segment) = self_path.segments.last() else {
            return Ok(Some(TraitApplicability::Maybe));
        };
        self.match_generic_args(
            trait_impl,
            impl_data,
            &segment.args,
            &goal_ty.args,
            table,
            subst,
        )
    }

    fn unknown_self_applicability(self_ty: &InferTy) -> Option<TraitApplicability> {
        match self_ty {
            // A bare variable could match many impls for the same trait. Returning every impl as a
            // maybe-candidate would be noisy and not useful for commit mode, so leave it unsolved.
            InferTy::Var(_) | InferTy::IntegerVar(_) | InferTy::FloatVar(_) => None,
            InferTy::Unknown | InferTy::Syntax(_) => Some(TraitApplicability::Maybe),
            InferTy::Unit
            | InferTy::Never
            | InferTy::Primitive(_)
            | InferTy::Tuple(_)
            | InferTy::Array { .. }
            | InferTy::Slice(_)
            | InferTy::Reference { .. }
            | InferTy::Opaque { .. }
            | InferTy::Nominal(_)
            | InferTy::SelfTy(_) => None,
        }
    }

    fn match_trait_args(
        &self,
        goal: &TraitGoal,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        table: &mut InferenceTable,
        subst: &mut InferTypeSubst,
    ) -> Result<Option<TraitApplicability>, I::Error> {
        let Some(TypeRef::Path(trait_path)) = impl_data.trait_ref.as_ref() else {
            return Ok(goal.args.is_empty().then_some(TraitApplicability::Maybe));
        };

        let impl_args = trait_path
            .segments
            .last()
            .map(|segment| segment.args.as_slice())
            .unwrap_or(&[]);
        self.match_generic_args(trait_impl, impl_data, impl_args, &goal.args, table, subst)
    }

    fn match_generic_args(
        &self,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        impl_args: &[ItemGenericArg],
        goal_args: &[InferGenericArg],
        table: &mut InferenceTable,
        subst: &mut InferTypeSubst,
    ) -> Result<Option<TraitApplicability>, I::Error> {
        if impl_args.len() != goal_args.len() {
            return Ok(None);
        }

        let mut applicability = TraitApplicability::Yes;
        for (impl_arg, goal_arg) in impl_args.iter().zip(goal_args) {
            let Some(arg_applicability) =
                self.match_generic_arg(trait_impl, impl_data, impl_arg, goal_arg, table, subst)?
            else {
                return Ok(None);
            };
            applicability = applicability.and(arg_applicability);
        }

        Ok(Some(applicability))
    }

    fn match_generic_arg(
        &self,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        impl_arg: &ItemGenericArg,
        goal_arg: &InferGenericArg,
        table: &mut InferenceTable,
        subst: &mut InferTypeSubst,
    ) -> Result<Option<TraitApplicability>, I::Error> {
        match (impl_arg, goal_arg) {
            (ItemGenericArg::Type(impl_ty), InferGenericArg::Type(goal_ty)) => {
                self.match_type_ref(trait_impl, impl_data, impl_ty, goal_ty, table, subst)
            }
            (ItemGenericArg::Lifetime(_), InferGenericArg::Lifetime(_)) => {
                Ok(Some(TraitApplicability::Yes))
            }
            (ItemGenericArg::Const(lhs), InferGenericArg::Const(rhs)) if lhs == rhs => {
                Ok(Some(TraitApplicability::Yes))
            }
            (
                ItemGenericArg::FnTraitArgs {
                    params: impl_params,
                    ret: impl_ret,
                },
                InferGenericArg::FnTraitArgs {
                    params: goal_params,
                    ret: goal_ret,
                },
            ) if impl_params.len() == goal_params.len() => {
                let mut applicability = TraitApplicability::Yes;
                for (impl_param, goal_param) in impl_params.iter().zip(goal_params) {
                    let Some(param_applicability) = self.match_type_ref(
                        trait_impl, impl_data, impl_param, goal_param, table, subst,
                    )?
                    else {
                        return Ok(None);
                    };
                    applicability = applicability.and(param_applicability);
                }
                let Some(ret_applicability) =
                    self.match_type_ref(trait_impl, impl_data, impl_ret, goal_ret, table, subst)?
                else {
                    return Ok(None);
                };
                Ok(Some(applicability.and(ret_applicability)))
            }
            (
                ItemGenericArg::AssocType {
                    name: impl_name,
                    ty: impl_ty,
                },
                InferGenericArg::AssocType {
                    name: goal_name,
                    ty: goal_ty,
                },
            ) if impl_name == goal_name => match (impl_ty, goal_ty) {
                (Some(impl_ty), Some(goal_ty)) => {
                    self.match_type_ref(trait_impl, impl_data, impl_ty, goal_ty, table, subst)
                }
                (None, None) => Ok(Some(TraitApplicability::Yes)),
                (Some(_), None) | (None, Some(_)) => Ok(Some(TraitApplicability::Maybe)),
            },
            _ => Ok(None),
        }
    }

    fn match_type_ref(
        &self,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        impl_ty: &TypeRef,
        goal_ty: &InferTy,
        table: &mut InferenceTable,
        subst: &mut InferTypeSubst,
    ) -> Result<Option<TraitApplicability>, I::Error> {
        if let Some(name) = impl_ty.type_param_name()
            && Self::is_impl_type_param(&impl_data.generics, &name)
        {
            return match subst.try_push(table, name, goal_ty.clone()) {
                Ok(()) => Ok(Some(TraitApplicability::Yes)),
                Err(InferenceConflict) => Ok(None),
            };
        }

        let goal_ty = table.resolve_root_var(goal_ty);
        let mut applicability = TraitApplicability::Yes;
        if Self::type_is_uncertain(&goal_ty) {
            applicability = TraitApplicability::Maybe;
        }

        match (impl_ty, &goal_ty) {
            (TypeRef::Unit, InferTy::Unit) | (TypeRef::Never, InferTy::Never) => {
                Ok(Some(applicability))
            }
            (TypeRef::Tuple(impl_fields), InferTy::Tuple(goal_fields))
                if impl_fields.len() == goal_fields.len() =>
            {
                for (impl_field, goal_field) in impl_fields.iter().zip(goal_fields) {
                    let Some(field_applicability) = self.match_type_ref(
                        trait_impl, impl_data, impl_field, goal_field, table, subst,
                    )?
                    else {
                        return Ok(None);
                    };
                    applicability = applicability.and(field_applicability);
                }
                Ok(Some(applicability))
            }
            (
                TypeRef::Array {
                    inner: impl_inner,
                    len: impl_len,
                },
                InferTy::Array {
                    inner: goal_inner,
                    len: goal_len,
                },
            ) if impl_len == goal_len => {
                self.match_type_ref(trait_impl, impl_data, impl_inner, goal_inner, table, subst)
            }
            (TypeRef::Slice(impl_inner), InferTy::Slice(goal_inner)) => {
                self.match_type_ref(trait_impl, impl_data, impl_inner, goal_inner, table, subst)
            }
            (
                TypeRef::Reference {
                    mutability,
                    inner: impl_inner,
                    ..
                },
                InferTy::Reference {
                    mutability: goal_mutability,
                    inner: goal_inner,
                },
            ) if *mutability == *goal_mutability => {
                self.match_type_ref(trait_impl, impl_data, impl_inner, goal_inner, table, subst)
            }
            (TypeRef::Path(_), _)
                if Self::type_ref_mentions_impl_type_param(impl_ty, &impl_data.generics) =>
            {
                Ok(None)
            }
            (TypeRef::Path(_), _) => {
                let context = TypePathContext {
                    module: impl_data.owner,
                    impl_ref: Some(trait_impl.impl_ref),
                };
                let resolved_ty = self.item_paths.resolve_type_ref(
                    impl_ty,
                    context,
                    Ty::syntax(impl_ty.clone()),
                    &TypeSubst::new(),
                )?;
                if !resolved_ty.is_projectable() {
                    return Ok(Some(TraitApplicability::Maybe));
                }

                match table.try_unify(&InferTy::from_ty(&resolved_ty), &goal_ty) {
                    Ok(()) => Ok(Some(applicability)),
                    Err(InferenceConflict) => Ok(None),
                }
            }
            (TypeRef::Unknown(_) | TypeRef::Infer, _) => Ok(Some(TraitApplicability::Maybe)),
            (
                TypeRef::RawPointer { .. }
                | TypeRef::FnPointer { .. }
                | TypeRef::ImplTrait(_)
                | TypeRef::DynTrait(_),
                _,
            ) => Ok(None),
            _ => Ok(None),
        }
    }

    fn type_is_uncertain(ty: &InferTy) -> bool {
        match ty {
            InferTy::Unknown | InferTy::Syntax(_) => true,
            InferTy::Tuple(fields) => fields.iter().any(Self::type_is_uncertain),
            InferTy::Array { inner, .. }
            | InferTy::Slice(inner)
            | InferTy::Reference { inner, .. } => Self::type_is_uncertain(inner),
            InferTy::Opaque { .. } => true,
            InferTy::Var(_)
            | InferTy::IntegerVar(_)
            | InferTy::FloatVar(_)
            | InferTy::Unit
            | InferTy::Never
            | InferTy::Primitive(_)
            | InferTy::Nominal(_)
            | InferTy::SelfTy(_) => false,
        }
    }

    fn is_impl_type_param(generics: &GenericParams, name: &Name) -> bool {
        generics.type_param_names().any(|param| param == name)
    }

    fn type_ref_mentions_impl_type_param(ty: &TypeRef, generics: &GenericParams) -> bool {
        match ty {
            TypeRef::Path(path) => path.segments.iter().any(|segment| {
                Self::is_impl_type_param(generics, &segment.name)
                    || segment
                        .args
                        .iter()
                        .any(|arg| Self::generic_arg_mentions_impl_type_param(arg, generics))
            }),
            TypeRef::Tuple(types) => types
                .iter()
                .any(|ty| Self::type_ref_mentions_impl_type_param(ty, generics)),
            TypeRef::Reference { inner, .. }
            | TypeRef::RawPointer { inner, .. }
            | TypeRef::Slice(inner)
            | TypeRef::Array { inner, .. } => {
                Self::type_ref_mentions_impl_type_param(inner, generics)
            }
            TypeRef::FnPointer { params, ret } => {
                params
                    .iter()
                    .any(|ty| Self::type_ref_mentions_impl_type_param(ty, generics))
                    || Self::type_ref_mentions_impl_type_param(ret, generics)
            }
            TypeRef::ImplTrait(bounds) | TypeRef::DynTrait(bounds) => bounds
                .iter()
                .any(|bound| Self::type_bound_mentions_impl_type_param(bound, generics)),
            TypeRef::Unknown(_) | TypeRef::Never | TypeRef::Unit | TypeRef::Infer => false,
        }
    }

    fn generic_arg_mentions_impl_type_param(
        arg: &ItemGenericArg,
        generics: &GenericParams,
    ) -> bool {
        match arg {
            ItemGenericArg::Type(ty) => Self::type_ref_mentions_impl_type_param(ty, generics),
            ItemGenericArg::AssocType { ty, .. } => ty
                .as_ref()
                .is_some_and(|ty| Self::type_ref_mentions_impl_type_param(ty, generics)),
            ItemGenericArg::FnTraitArgs { params, ret } => {
                params
                    .iter()
                    .any(|ty| Self::type_ref_mentions_impl_type_param(ty, generics))
                    || Self::type_ref_mentions_impl_type_param(ret, generics)
            }
            ItemGenericArg::Lifetime(_)
            | ItemGenericArg::Const(_)
            | ItemGenericArg::Unsupported(_) => false,
        }
    }

    fn type_bound_mentions_impl_type_param(bound: &TypeBound, generics: &GenericParams) -> bool {
        match bound {
            TypeBound::Trait(ty) => Self::type_ref_mentions_impl_type_param(ty, generics),
            TypeBound::Lifetime(_) | TypeBound::Unsupported(_) => false,
        }
    }
}
