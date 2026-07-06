//! Impl-header matching for trait selection candidates.
//!
//! This is the cheap, local half of trait selection. It does not try to prove where-clauses or
//! type-param bounds; that is either delegated to Chalk or to a body-local obligation path. The job
//! here is narrower:
//!
//! - compare one visible impl header with a `TraitGoal`;
//! - bind direct impl type params such as `impl<T> FromIterator<T> for Vec<T>`;
//! - write any equality evidence into a trial inference table;
//! - report whether the header is definite, maybe-applicable, or not applicable.
//!
//! Keeping this layer small is important because many callers probe candidates before they know
//! whether they want to commit the result. A match here should mean "this impl header can be tried",
//! not "the whole impl is proven".

use rg_ir_model::hir::items::ImplData;
use rg_ir_model::items::{GenericArg as ItemGenericArg, GenericParams, TypeBound, TypeRef};
use rg_ir_model::{TraitApplicability, TraitImplRef, TypeDefRef};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_text::Name;

use super::TraitGoal;
use crate::ItemPathQuery;
use crate::generic_arg::item_generic_args_align;
use crate::inference::{InferenceConflict, InferenceTable, InferenceTypeSubst};
use crate::{GenericArg, Ty, TypeSubst};

/// Matches a single impl header against a trait goal.
///
/// The matcher is intentionally state-light: callers pass in the trial table and substitution that
/// should receive evidence. This lets `TraitSelectionQuery` clone the caller table per candidate
/// and commit only after uniqueness is known.
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

    /// Match both sides of an impl header against the goal.
    ///
    /// For `Vec<?T>: FromIterator<User>` and `impl<T> FromIterator<T> for Vec<T>`, this records
    /// `T = User` in `subst` and `?T = User` in the trial table. Impl predicates, such as
    /// `T: Clone`, are intentionally checked after this function by the caller's policy.
    pub(super) fn match_goal(
        &self,
        goal: &TraitGoal,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        table: &mut InferenceTable,
        subst: &mut InferenceTypeSubst,
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

    /// Match the impl's `Self` type before looking at trait args.
    ///
    /// The self type is usually the strongest filter. A nominal self type can reject most impls by
    /// definition id, while a blanket self param such as `impl<I: Iterator> IntoIterator for I`
    /// only binds `I` and leaves the `I: Iterator` proof for the predicate stage.
    fn match_self_ty(
        &self,
        goal: &TraitGoal,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        table: &mut InferenceTable,
        subst: &mut InferenceTypeSubst,
    ) -> Result<Option<TraitApplicability>, I::Error> {
        if let Some(self_def) = impl_data.resolved_self_ty.as_option() {
            return self
                .match_nominal_self_ty(goal, trait_impl, *self_def, impl_data, table, subst);
        }

        if impl_data
            .self_ty
            .type_param_name()
            .is_some_and(|name| Self::is_impl_type_param(&impl_data.generics, &name))
        {
            // Blanket self params are only the header match, for example
            // `impl<I: Iterator> IntoIterator for I`. Predicate checking below still has to prove
            // the bound after `I` is bound to the concrete goal self type.
            return Self::match_type_param_self_ty(goal, impl_data, table, subst);
        }

        if let Some(self_def) = self.resolve_nominal_self_def(trait_impl, impl_data)? {
            return self.match_nominal_self_ty(goal, trait_impl, self_def, impl_data, table, subst);
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

    /// Bind a blanket impl self param to the goal receiver.
    ///
    /// Example: for `impl<I: Iterator> IntoIterator for I`, the header match for
    /// `Iter<User>: IntoIterator` records `I = Iter<User>`. This is not enough by itself; the
    /// later predicate step still has to prove `Iter<User>: Iterator`.
    fn match_type_param_self_ty(
        goal: &TraitGoal,
        impl_data: &ImplData,
        table: &mut InferenceTable,
        subst: &mut InferenceTypeSubst,
    ) -> Result<Option<TraitApplicability>, I::Error> {
        let Some(name) = impl_data.self_ty.type_param_name() else {
            return Ok(None);
        };
        let self_ty = table.resolve_root_var(&goal.self_ty);
        let applicability = match &self_ty {
            Ty::InferVar { .. } => return Ok(None),
            // `impl Trait` hides the concrete type, but it is still one concrete type from the
            // caller's point of view. Bind the blanket self param and let predicate checking prove
            // any required bounds from the opaque trait list.
            Ty::Opaque { .. } => TraitApplicability::Yes,
            _ => {
                let Some(applicability) =
                    Self::unknown_self_applicability(&self_ty).or_else(|| {
                        (!Self::type_is_uncertain(&self_ty)).then_some(TraitApplicability::Yes)
                    })
                else {
                    return Ok(None);
                };
                applicability
            }
        };

        match subst.try_push(table, name, goal.self_ty.clone()) {
            Ok(()) => Ok(Some(applicability)),
            Err(InferenceConflict) => Ok(None),
        }
    }

    /// Resolve a path self type when semantic-ir did not already give us its type definition.
    ///
    /// Most impls have `resolved_self_ty`, but generated or partially-supported headers may only
    /// have the written `TypeRef`. This fallback lets ordinary paths like `Vec<T>` still use the
    /// fast nominal matching path after resolution succeeds.
    fn resolve_nominal_self_def(
        &self,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
    ) -> Result<Option<TypeDefRef>, I::Error> {
        let TypeRef::Path(_) = &impl_data.self_ty else {
            return Ok(None);
        };

        let context = TypePathContext {
            module: impl_data.owner,
            impl_ref: Some(trait_impl.impl_ref),
        };
        let resolved_ty = self.item_paths.resolve_type_ref(
            &impl_data.self_ty,
            context,
            Ty::syntax(impl_data.self_ty.clone()),
            &TypeSubst::new(),
        )?;

        match resolved_ty {
            Ty::Nominal(ty) | Ty::SelfTy(ty) => Ok(Some(ty.def)),
            Ty::Unit
            | Ty::Never
            | Ty::Primitive(_)
            | Ty::Tuple(_)
            | Ty::Array { .. }
            | Ty::Slice(_)
            | Ty::Reference { .. }
            | Ty::Opaque { .. }
            | Ty::Closure(_)
            | Ty::FunctionItem(_)
            | Ty::Syntax(_)
            | Ty::Unknown
            | Ty::InferVar { .. } => Ok(None),
        }
    }

    /// Match a nominal impl self type and then line up its generic args.
    ///
    /// Once the definition id matches, the interesting part is the substitution: `Vec<T>` against
    /// `Vec<User>` binds `T = User`, while `Vec<String>` against `Vec<User>` rejects the impl.
    fn match_nominal_self_ty(
        &self,
        goal: &TraitGoal,
        trait_impl: TraitImplRef,
        self_def: TypeDefRef,
        impl_data: &ImplData,
        table: &mut InferenceTable,
        subst: &mut InferenceTypeSubst,
    ) -> Result<Option<TraitApplicability>, I::Error> {
        let self_ty = table.resolve_root_var(&goal.self_ty);
        let (Ty::Nominal(goal_ty) | Ty::SelfTy(goal_ty)) = &self_ty else {
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

    /// Decide whether an unknown-looking receiver should stay as a maybe candidate.
    ///
    /// `Ty::Unknown` and unresolved syntax can be useful for exploratory UI queries. Plain
    /// inference vars are different: a bare `?T: Trait` would make nearly every impl a maybe
    /// candidate, so we leave that to later evidence instead of flooding selection.
    fn unknown_self_applicability(self_ty: &Ty) -> Option<TraitApplicability> {
        match self_ty {
            // A bare variable could match many impls for the same trait. Returning every impl as a
            // maybe-candidate would be noisy and not useful for commit mode, so leave it unsolved.
            Ty::InferVar { .. } => None,
            Ty::Unknown | Ty::Syntax(_) => Some(TraitApplicability::Maybe),
            Ty::Unit
            | Ty::Never
            | Ty::Primitive(_)
            | Ty::Tuple(_)
            | Ty::Array { .. }
            | Ty::Slice(_)
            | Ty::Reference { .. }
            | Ty::Opaque { .. }
            | Ty::Closure(_)
            | Ty::FunctionItem(_)
            | Ty::Nominal(_)
            | Ty::SelfTy(_) => None,
        }
    }

    /// Match trait path arguments after the receiver side was accepted.
    ///
    /// For `impl<T> FromIterator<T> for Vec<T>`, this compares the impl's `T` with the goal's
    /// `User` in `Vec<?T>: FromIterator<User>`. Associated equality args, such as
    /// `Iterator<Item = User>`, are not positional trait inputs; the selection query checks them
    /// by projecting the selected associated type after the cheap header match succeeds.
    fn match_trait_args(
        &self,
        goal: &TraitGoal,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        table: &mut InferenceTable,
        subst: &mut InferenceTypeSubst,
    ) -> Result<Option<TraitApplicability>, I::Error> {
        let Some(TypeRef::Path(trait_path)) = impl_data.trait_ref.as_ref() else {
            return Ok(goal
                .iter_positional_args()
                .next()
                .is_none()
                .then_some(TraitApplicability::Maybe));
        };

        let impl_args = trait_path
            .segments
            .last()
            .into_iter()
            .flat_map(|segment| segment.args.iter())
            .filter(|arg| !matches!(arg, ItemGenericArg::AssocType { .. }));
        self.match_generic_args(
            trait_impl,
            impl_data,
            impl_args,
            goal.iter_positional_args(),
            table,
            subst,
        )
    }

    /// Match generic args position-by-position, collecting the weakest applicability.
    ///
    /// Lifetime parameter alignment is delegated to `item_generic_args_align`; this method only
    /// owns the trait-selection policy for each non-lifetime argument pair.
    fn match_generic_args<'impl_arg, 'goal_arg>(
        &self,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        impl_args: impl IntoIterator<Item = &'impl_arg ItemGenericArg>,
        goal_args: impl IntoIterator<Item = &'goal_arg GenericArg>,
        table: &mut InferenceTable,
        subst: &mut InferenceTypeSubst,
    ) -> Result<Option<TraitApplicability>, I::Error> {
        let mut applicability = TraitApplicability::Yes;
        let impl_lifetime_params = impl_data
            .generics
            .lifetimes
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>();

        let matched = item_generic_args_align(
            impl_args,
            goal_args,
            &impl_lifetime_params,
            |impl_arg, goal_arg| {
                let Some(arg_applicability) = self.non_lifetime_generic_arg_applicability(
                    trait_impl, impl_data, impl_arg, goal_arg, table, subst,
                )?
                else {
                    return Ok(false);
                };
                applicability = applicability.and(arg_applicability);
                Ok(true)
            },
        )?;

        if !matched {
            return Ok(None);
        }

        Ok(Some(applicability))
    }

    /// Match one non-lifetime generic arg in a trait path or nominal self type.
    ///
    /// Function-trait args and associated-type equality args can appear in bounds such as
    /// `FnOnce(T) -> R` or `Iterator<Item = T>`, so the matcher handles the simple structural
    /// forms even though it does not perform general trait solving here.
    fn non_lifetime_generic_arg_applicability(
        &self,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        impl_arg: &ItemGenericArg,
        goal_arg: &GenericArg,
        table: &mut InferenceTable,
        subst: &mut InferenceTypeSubst,
    ) -> Result<Option<TraitApplicability>, I::Error> {
        debug_assert!(
            !matches!(impl_arg, ItemGenericArg::Lifetime(_)),
            "item_generic_args_align consumes item-side lifetime args"
        );

        match (impl_arg, goal_arg) {
            (ItemGenericArg::Type(impl_ty), GenericArg::Type(goal_ty)) => {
                self.match_type_ref(trait_impl, impl_data, impl_ty, goal_ty, table, subst)
            }
            (ItemGenericArg::Const(lhs), GenericArg::Const(rhs)) if lhs == rhs => {
                Ok(Some(TraitApplicability::Yes))
            }
            (
                ItemGenericArg::FnTraitArgs {
                    params: impl_params,
                    ret: impl_ret,
                },
                GenericArg::FnTraitArgs {
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
                GenericArg::AssocType {
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

    /// Match written type syntax from an impl header against an inference type from the goal.
    ///
    /// This is the small unification-like core of header matching. Direct impl params bind into
    /// `subst`; concrete written types are resolved and unified with the goal; unsupported source
    /// shapes either become `Maybe` or reject the candidate, depending on whether using them would
    /// invent evidence.
    fn match_type_ref(
        &self,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        impl_ty: &TypeRef,
        goal_ty: &Ty,
        table: &mut InferenceTable,
        subst: &mut InferenceTypeSubst,
    ) -> Result<Option<TraitApplicability>, I::Error> {
        if let Some(name) = impl_ty.type_param_name()
            && Self::is_impl_type_param(&impl_data.generics, &name)
        {
            // A bare impl type param is the only binding operation this matcher performs directly.
            // More complex generic patterns are rejected below unless they resolve to concrete
            // projectable types.
            return match subst.try_push(table, name, goal_ty.clone()) {
                Ok(()) => Ok(Some(TraitApplicability::Yes)),
                Err(InferenceConflict) => Ok(None),
            };
        }

        let goal_ty = table.resolve_root_var(goal_ty);
        let mut applicability = TraitApplicability::Yes;
        if Self::type_is_uncertain(&goal_ty) {
            // Unknown or syntax-backed goal types can keep a candidate alive for exploratory
            // callers, but they should not be treated as a proven concrete match.
            applicability = TraitApplicability::Maybe;
        }

        match (impl_ty, &goal_ty) {
            (TypeRef::Unit, Ty::Unit) | (TypeRef::Never, Ty::Never) => Ok(Some(applicability)),
            (TypeRef::Tuple(impl_fields), Ty::Tuple(goal_fields))
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
                Ty::Array {
                    inner: goal_inner,
                    len: goal_len,
                },
            ) if Self::array_len_matches(impl_len, goal_len, &impl_data.generics) => {
                self.match_type_ref(trait_impl, impl_data, impl_inner, goal_inner, table, subst)
            }
            (TypeRef::Slice(impl_inner), Ty::Slice(goal_inner)) => {
                self.match_type_ref(trait_impl, impl_data, impl_inner, goal_inner, table, subst)
            }
            (
                TypeRef::Reference {
                    mutability,
                    inner: impl_inner,
                    ..
                },
                Ty::Reference {
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

                // Concrete type refs can use the normal inference-table unifier. This is how a
                // header like `impl Trait for Vec<User>` rejects `Vec<String>` without special
                // matching code for every nominal shape.
                match table.try_unify(&resolved_ty, &goal_ty) {
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

    /// Return true when a goal-side type is too incomplete for a definite header match.
    fn type_is_uncertain(ty: &Ty) -> bool {
        match ty {
            Ty::Unknown | Ty::Syntax(_) => true,
            Ty::Tuple(fields) => fields.iter().any(Self::type_is_uncertain),
            Ty::Array { inner, .. } | Ty::Slice(inner) | Ty::Reference { inner, .. } => {
                Self::type_is_uncertain(inner)
            }
            Ty::Opaque { .. } => true,
            Ty::InferVar { .. }
            | Ty::Unit
            | Ty::Never
            | Ty::Primitive(_)
            | Ty::Closure(_)
            | Ty::FunctionItem(_)
            | Ty::Nominal(_)
            | Ty::SelfTy(_) => false,
        }
    }

    /// Match array lengths without pretending to solve const generics.
    ///
    /// A concrete `impl<T> Trait for [T; 3]` only matches `[User; 3]`. A const param in the impl
    /// header, such as `[T; N]`, is accepted structurally because the rest of this matcher can
    /// still bind `T` without needing to know the actual value of `N`.
    fn array_len_matches(
        impl_len: &Option<String>,
        goal_len: &Option<String>,
        generics: &GenericParams,
    ) -> bool {
        match impl_len {
            Some(len)
                if generics
                    .consts
                    .iter()
                    .any(|param| param.name.as_str() == len.as_str()) =>
            {
                true
            }
            _ => impl_len == goal_len,
        }
    }

    /// Return true when the name belongs to the impl's own type params.
    fn is_impl_type_param(generics: &GenericParams, name: &Name) -> bool {
        generics.type_param_names().any(|param| param == name)
    }

    /// Detect nested impl-param patterns the header matcher cannot safely bind.
    ///
    /// `T` by itself can be bound directly. `Option<T>` is different: matching that against a goal
    /// type requires recursive decomposition after resolving `Option`, and this local matcher only
    /// supports that for already-concrete projectable types.
    fn type_ref_mentions_impl_type_param(ty: &TypeRef, generics: &GenericParams) -> bool {
        match ty {
            TypeRef::Path(path) => path.mentions_type_param(
                &generics
                    .type_param_names()
                    .map(|name| name.as_str())
                    .collect::<Vec<_>>(),
            ),
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

    fn type_bound_mentions_impl_type_param(bound: &TypeBound, generics: &GenericParams) -> bool {
        match bound {
            TypeBound::Trait(ty) => Self::type_ref_mentions_impl_type_param(ty, generics),
            TypeBound::Lifetime(_) | TypeBound::Unsupported(_) => false,
        }
    }
}
