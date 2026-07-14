//! Canonical impl-header matching for trait selection candidates.
//!
//! Source syntax is lowered before it reaches this module. Header matching therefore compares the
//! same `Ty` and `GenericArg` vocabulary used by inference and Chalk, and the only bindable values
//! are owner-scoped impl parameters.

use std::collections::HashMap;

use rg_ir_model::{FunctionRef, GenericDefRef, TraitApplicability, TraitImplRef, TypeDefRef};
use rg_std::UniqueVec;

use super::TraitGoal;
use crate::inference::{InferenceConflict, InferenceSubstitution, InferenceTable};
use crate::{
    ClosureTyId, ConstValue, GenericArg, ImplHeader, Lifetime, Mutability, PrimitiveTy, Ty,
};

/// Top-level semantic type fingerprint used to index trait impl headers.
///
/// `impl Trait for Vec<T>` can only match another `Vec<_>`, while `impl<T> Trait for T` must remain
/// available for every receiver. Associated aliases are also left unindexed because the bounded
/// matcher deliberately treats their hidden shape as uncertain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum TraitSelfHead {
    Unit,
    Never,
    Primitive(PrimitiveTy),
    Tuple(usize),
    Array,
    Slice,
    Reference(Mutability),
    RawPointer(Mutability),
    FnPointer(usize),
    Adt(TypeDefRef),
    Closure(ClosureTyId),
    FnDef(FunctionRef),
}

impl TraitSelfHead {
    pub(super) fn from_ty(ty: &Ty) -> Option<Self> {
        match ty {
            Ty::Unit => Some(Self::Unit),
            Ty::Never => Some(Self::Never),
            Ty::Primitive(primitive) => Some(Self::Primitive(*primitive)),
            Ty::Tuple(fields) => Some(Self::Tuple(fields.len())),
            Ty::Array { .. } => Some(Self::Array),
            Ty::Slice(_) => Some(Self::Slice),
            Ty::Reference { mutability, .. } => Some(Self::Reference(*mutability)),
            Ty::RawPointer { mutability, .. } => Some(Self::RawPointer(*mutability)),
            Ty::FnPointer { params, .. } => Some(Self::FnPointer(params.len())),
            Ty::Adt(ty) => Some(Self::Adt(ty.def)),
            Ty::Closure(id) => Some(Self::Closure(*id)),
            Ty::FnDef(function) => Some(Self::FnDef(function.def)),
            Ty::Param(_) | Ty::Alias(_) | Ty::Unknown | Ty::InferVar { .. } => None,
        }
    }
}

/// Visible trait impls partitioned by the top-level shape of their canonical `Self` type.
#[derive(Clone, Default)]
pub(super) struct TraitImplCandidateIndex {
    by_self_head: HashMap<TraitSelfHead, UniqueVec<TraitImplRef>>,
    fallbacks: UniqueVec<TraitImplRef>,
}

impl TraitImplCandidateIndex {
    pub(super) fn push(&mut self, trait_impl: TraitImplRef, header: &ImplHeader) {
        if let Some(head) = TraitSelfHead::from_ty(&header.self_ty) {
            self.by_self_head.entry(head).or_default().push(trait_impl);
        } else {
            self.fallbacks.push(trait_impl);
        }
    }

    pub(super) fn candidates(&self, head: TraitSelfHead) -> UniqueVec<TraitImplRef> {
        let mut candidates = self.by_self_head.get(&head).cloned().unwrap_or_default();
        candidates.extend(self.fallbacks.iter().copied());
        candidates
    }
}

/// Matches a single lowered impl header against a trait goal.
pub(super) struct CandidateMatcher;

impl CandidateMatcher {
    /// Match the canonical self type and positional trait arguments, recording impl-parameter
    /// evidence in the candidate's trial substitution.
    pub(super) fn match_goal(
        &self,
        goal: &TraitGoal,
        trait_impl: TraitImplRef,
        header: &ImplHeader,
        table: &mut InferenceTable,
        subst: &mut InferenceSubstitution,
    ) -> Option<TraitApplicability> {
        if header.owner != trait_impl.impl_ref {
            return None;
        }
        let Some(trait_ref) = &header.trait_ref else {
            return None;
        };
        if trait_ref.application.def != trait_impl.trait_ref {
            return None;
        }

        let owner = GenericDefRef::Impl(trait_impl.impl_ref);
        // A bare inference receiver must not make every blanket `impl<T> Trait for T` a viable
        // candidate. Nested variables are different: `Vec<?T>` against `Vec<T>` is useful direct
        // evidence and must be allowed to flow into the candidate substitution.
        if matches!(&header.self_ty, Ty::Param(param) if param.owner == owner)
            && matches!(table.resolve_root_var(goal.self_ty()), Ty::InferVar { .. })
        {
            return None;
        }
        let self_applicability =
            self.match_ty(owner, &header.self_ty, goal.self_ty(), table, subst)?;

        // The canonical trait application stores `Self` first. It was matched above because the
        // impl self type carries the useful structural pattern; compare only the remaining inputs
        // here. Erased/omitted lifetime positions do not shift following type or const arguments.
        let header_args = trait_ref.application.args.iter().skip(1);
        let goal_args = goal.iter_positional_args();
        let args_applicability = self.match_args(owner, header_args, goal_args, table, subst)?;

        Some(self_applicability.and(args_applicability))
    }

    fn match_args<'pattern, 'evidence>(
        &self,
        owner: GenericDefRef,
        patterns: impl IntoIterator<Item = &'pattern GenericArg>,
        evidence: impl IntoIterator<Item = &'evidence GenericArg>,
        table: &mut InferenceTable,
        subst: &mut InferenceSubstitution,
    ) -> Option<TraitApplicability> {
        let patterns = patterns.into_iter();
        let mut evidence = evidence.into_iter().peekable();
        let mut applicability = TraitApplicability::Yes;

        for pattern in patterns {
            // Lifetime arguments may be omitted at a use site. Their semantic slot still exists
            // in the impl application, but it must not consume a following type argument.
            if matches!(pattern, GenericArg::Lifetime(_))
                && !matches!(evidence.peek(), Some(GenericArg::Lifetime(_)))
            {
                if let GenericArg::Lifetime(Lifetime::Param(param)) = pattern
                    && param.owner == owner
                {
                    // Region inference is outside this table. Retain an erased binding so an
                    // omitted use-site lifetime cannot leave the impl's own parameter behind.
                    subst.bind_lifetime(Lifetime::Param(*param), Lifetime::Erased);
                }
                continue;
            }
            let evidence = evidence.next()?;
            let arg_applicability = self.match_arg(owner, pattern, evidence, table, subst)?;
            applicability = applicability.and(arg_applicability);
        }

        evidence.next().is_none().then_some(applicability)
    }

    fn match_arg(
        &self,
        owner: GenericDefRef,
        pattern: &GenericArg,
        evidence: &GenericArg,
        table: &mut InferenceTable,
        subst: &mut InferenceSubstitution,
    ) -> Option<TraitApplicability> {
        match (pattern, evidence) {
            (GenericArg::Type(pattern), GenericArg::Type(evidence)) => {
                self.match_ty(owner, pattern, evidence, table, subst)
            }
            (GenericArg::Lifetime(pattern), GenericArg::Lifetime(evidence)) => {
                self.match_lifetime(owner, *pattern, *evidence, subst)
            }
            (GenericArg::Const(pattern), GenericArg::Const(evidence)) => {
                self.match_const(owner, *pattern, *evidence, subst)
            }
            (GenericArg::Type(_), GenericArg::Lifetime(_) | GenericArg::Const(_))
            | (GenericArg::Lifetime(_), GenericArg::Type(_) | GenericArg::Const(_))
            | (GenericArg::Const(_), GenericArg::Type(_) | GenericArg::Lifetime(_)) => None,
        }
    }

    fn match_ty(
        &self,
        owner: GenericDefRef,
        pattern: &Ty,
        evidence: &Ty,
        table: &mut InferenceTable,
        subst: &mut InferenceSubstitution,
    ) -> Option<TraitApplicability> {
        let evidence = table.resolve_root_var(evidence);
        if let Ty::Param(param) = pattern
            && param.owner == owner
        {
            return match subst.try_push_type(table, *param, evidence) {
                Ok(()) => Some(TraitApplicability::Yes),
                Err(InferenceConflict) => None,
            };
        }

        if matches!(pattern, Ty::Unknown) || matches!(evidence, Ty::Unknown) {
            return Some(TraitApplicability::Maybe);
        }

        let mut applicability = TraitApplicability::Yes;
        let matched = match (pattern, &evidence) {
            (Ty::Unit, Ty::Unit)
            | (Ty::Never, Ty::Never)
            | (Ty::Primitive(_), Ty::Primitive(_))
            | (Ty::Closure(_), Ty::Closure(_)) => pattern == &evidence,
            (Ty::Tuple(pattern), Ty::Tuple(evidence)) if pattern.len() == evidence.len() => {
                for (pattern, evidence) in pattern.iter().zip(evidence) {
                    applicability =
                        applicability.and(self.match_ty(owner, pattern, evidence, table, subst)?);
                }
                true
            }
            (
                Ty::Array {
                    inner: pattern,
                    len: pattern_len,
                },
                Ty::Array {
                    inner: evidence,
                    len: evidence_len,
                },
            ) => {
                applicability = applicability.and(self.match_const(
                    owner,
                    *pattern_len,
                    *evidence_len,
                    subst,
                )?);
                applicability =
                    applicability.and(self.match_ty(owner, pattern, evidence, table, subst)?);
                true
            }
            (Ty::Slice(pattern), Ty::Slice(evidence)) => {
                applicability =
                    applicability.and(self.match_ty(owner, pattern, evidence, table, subst)?);
                true
            }
            (
                Ty::Reference {
                    lifetime: pattern_lifetime,
                    mutability: pattern_mutability,
                    inner: pattern,
                },
                Ty::Reference {
                    lifetime: evidence_lifetime,
                    mutability: evidence_mutability,
                    inner: evidence,
                },
            ) if pattern_mutability == evidence_mutability => {
                applicability = applicability.and(self.match_lifetime(
                    owner,
                    *pattern_lifetime,
                    *evidence_lifetime,
                    subst,
                )?);
                applicability =
                    applicability.and(self.match_ty(owner, pattern, evidence, table, subst)?);
                true
            }
            (
                Ty::RawPointer {
                    mutability: pattern_mutability,
                    inner: pattern,
                },
                Ty::RawPointer {
                    mutability: evidence_mutability,
                    inner: evidence,
                },
            ) if pattern_mutability == evidence_mutability => {
                applicability =
                    applicability.and(self.match_ty(owner, pattern, evidence, table, subst)?);
                true
            }
            (
                Ty::FnPointer {
                    params: pattern_params,
                    ret: pattern_ret,
                },
                Ty::FnPointer {
                    params: evidence_params,
                    ret: evidence_ret,
                },
            ) if pattern_params.len() == evidence_params.len() => {
                for (pattern, evidence) in pattern_params.iter().zip(evidence_params) {
                    applicability =
                        applicability.and(self.match_ty(owner, pattern, evidence, table, subst)?);
                }
                applicability = applicability.and(self.match_ty(
                    owner,
                    pattern_ret,
                    evidence_ret,
                    table,
                    subst,
                )?);
                true
            }
            (Ty::Adt(pattern), Ty::Adt(evidence)) if pattern.def == evidence.def => {
                applicability = applicability.and(self.match_args(
                    owner,
                    &pattern.args,
                    &evidence.args,
                    table,
                    subst,
                )?);
                true
            }
            (Ty::FnDef(pattern), Ty::FnDef(evidence)) if pattern.def == evidence.def => {
                applicability = applicability.and(self.match_args(
                    owner,
                    &pattern.args,
                    &evidence.args,
                    table,
                    subst,
                )?);
                true
            }
            (Ty::Alias(pattern), Ty::Alias(evidence)) if pattern.same_definition(evidence) => {
                applicability = applicability.and(self.match_args(
                    owner,
                    pattern.args(),
                    evidence.args(),
                    table,
                    subst,
                )?);
                true
            }
            // Opaque/projection evidence is a real semantic type but may hide the concrete shape
            // required by this impl header. Keep it speculative instead of inventing equality.
            (_, Ty::Alias(_)) | (Ty::Alias(_), _) | (Ty::Param(_), _) => {
                applicability = TraitApplicability::Maybe;
                true
            }
            (_, Ty::InferVar { .. }) => {
                return match table.try_unify(pattern, &evidence) {
                    Ok(()) => Some(applicability),
                    Err(InferenceConflict) => None,
                };
            }
            _ => false,
        };

        matched.then_some(applicability)
    }

    fn match_lifetime(
        &self,
        owner: GenericDefRef,
        pattern: Lifetime,
        evidence: Lifetime,
        subst: &mut InferenceSubstitution,
    ) -> Option<TraitApplicability> {
        if let Lifetime::Param(param) = pattern
            && param.owner == owner
        {
            subst.bind_lifetime(pattern, evidence);
            return Some(TraitApplicability::Yes);
        }

        Some(match (pattern, evidence) {
            (Lifetime::Static, Lifetime::Static) => TraitApplicability::Yes,
            (Lifetime::Static, _) | (_, Lifetime::Static) => return None,
            (Lifetime::Param(_) | Lifetime::Erased, _) => TraitApplicability::Yes,
        })
    }

    fn match_const(
        &self,
        owner: GenericDefRef,
        pattern: ConstValue,
        evidence: ConstValue,
        subst: &mut InferenceSubstitution,
    ) -> Option<TraitApplicability> {
        let bindable = matches!(pattern, ConstValue::Param(param) if param.owner == owner);
        let compared_pattern = match pattern {
            ConstValue::Param(param) if bindable => subst.const_param(param).unwrap_or(pattern),
            _ => pattern,
        };
        let applicability = match (compared_pattern, evidence) {
            (ConstValue::Scalar(lhs), ConstValue::Scalar(rhs)) => {
                (lhs == rhs).then_some(TraitApplicability::Yes)
            }
            (ConstValue::Unknown, _) | (_, ConstValue::Unknown) => Some(TraitApplicability::Maybe),
            (ConstValue::Param(_), _) | (_, ConstValue::Param(_)) => Some(TraitApplicability::Yes),
        }?;
        if bindable {
            subst.bind_const(pattern, evidence);
        }
        Some(applicability)
    }
}

#[cfg(test)]
mod tests {
    use rg_ir_model::{
        ConstParamRef, CrateId, CrateRef, DefMapRef, GenericDefRef, ImplId, ImplRef,
        LifetimeParamRef, LocalLifetimeParamId, LocalTypeOrConstParamId, PackageSlot, TraitDefRef,
        TraitId, TraitImplRef,
    };

    use super::CandidateMatcher;
    use crate::inference::{InferenceSubstitution, InferenceTable};
    use crate::{
        ConstValue, GenericArg, ImplHeader, Lifetime, Mutability, PrimitiveTy, TraitApplication,
        TraitGoal, TraitRefLowering, Ty,
    };

    #[test]
    fn retains_const_and_lifetime_impl_evidence() {
        let origin = DefMapRef::Crate(CrateRef {
            package: PackageSlot(0),
            crate_id: CrateId(0),
        });
        let impl_ref = ImplRef::new(origin, ImplId(0));
        let owner = GenericDefRef::Impl(impl_ref);
        let lifetime_param = LifetimeParamRef {
            owner,
            local_id: LocalLifetimeParamId(0),
        };
        let const_param = ConstParamRef {
            owner,
            local_id: LocalTypeOrConstParamId(0),
        };
        let trait_ref = TraitDefRef::new(origin, TraitId(0));

        // This is the semantic form of an impl headed by `&'a [(); N]`. Both parameters must be
        // carried into the selected substitution because an associated value may mention either.
        let pattern = Ty::reference_with_lifetime(
            Lifetime::Param(lifetime_param),
            Mutability::Shared,
            Ty::array(Ty::Unit, ConstValue::Param(const_param)),
        );
        let evidence = Ty::reference_with_lifetime(
            Lifetime::Static,
            Mutability::Shared,
            Ty::array(Ty::Unit, ConstValue::Scalar(4)),
        );
        let header = ImplHeader {
            owner: impl_ref,
            self_ty: pattern.clone(),
            trait_ref: Some(TraitRefLowering {
                application: TraitApplication {
                    def: trait_ref,
                    args: vec![GenericArg::Type(Box::new(pattern.clone()))].into(),
                },
                associated_types: Vec::new(),
            }),
            clauses: Vec::new(),
        };
        let trait_impl = TraitImplRef {
            impl_ref,
            trait_ref,
        };
        let goal = TraitGoal::new(evidence.clone(), trait_ref, Vec::new());
        let mut table = InferenceTable::new();
        let mut subst = InferenceSubstitution::new();

        let applicability =
            CandidateMatcher.match_goal(&goal, trait_impl, &header, &mut table, &mut subst);

        assert_eq!(applicability, Some(rg_ir_model::TraitApplicability::Yes));
        assert_eq!(subst.as_substitution().apply(&pattern), evidence);
        assert_eq!(
            subst.as_substitution().apply(&Ty::reference_with_lifetime(
                Lifetime::Param(lifetime_param),
                Mutability::Shared,
                Ty::array(
                    Ty::Primitive(PrimitiveTy::Bool),
                    ConstValue::Param(const_param),
                ),
            )),
            Ty::reference_with_lifetime(
                Lifetime::Static,
                Mutability::Shared,
                Ty::array(Ty::Primitive(PrimitiveTy::Bool), ConstValue::Scalar(4),),
            )
        );
    }
}
