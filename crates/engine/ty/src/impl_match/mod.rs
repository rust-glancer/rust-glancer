//! Canonical impl-header matching for receiver-based item queries.
//!
//! Every entry point consumes canonical `ImplHeader` values from the signature-lowering boundary.
//! Syntax HIR remains available to declaration views, but receiver matching never interprets a
//! `TypeRef` itself.

mod receiver;
mod trait_impl;

pub use self::receiver::{InherentImplMatch, ReceiverFunctionCandidate, ReceiverImplMatches};

use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, GenericParamRef, ImplRef, TraitApplicability};
use rg_semantic_ir::ItemStoreSource;

use crate::{
    ConstValue, GenericArg, ImplHeader, ItemPathQuery, Lifetime, Substitution, Ty, TyContext,
    TypePathResolver,
};

/// Matcher for canonical impl headers stored in semantic item stores.
pub struct ImplMatcher<'query, D, I, R = ItemPathQuery<'query, D, I>> {
    context: TyContext<'query, D, I>,
    resolver: R,
}

impl<'query, D, I> ImplMatcher<'query, D, I, ItemPathQuery<'query, D, I>>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
{
    pub fn new(context: TyContext<'query, D, I>) -> Self {
        let resolver = context.item_paths().clone();
        Self { context, resolver }
    }
}

impl<'query, D, I, R> ImplMatcher<'query, D, I, R>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    pub fn with_resolver(context: TyContext<'query, D, I>, resolver: R) -> Self {
        Self { context, resolver }
    }

    /// Return the canonical header used by every matching operation.
    pub fn impl_header(&self, impl_ref: ImplRef) -> Result<Option<ImplHeader>, D::Error> {
        Ok(self
            .context
            .trait_selection()
            .impl_header_with(self.context.item_paths(), &self.resolver, impl_ref)?
            .map(|header| (*header).clone()))
    }

    /// Match the impl's semantic `Self` pattern and return owner-scoped bindings.
    pub fn impl_self_subst_for_impl(
        &self,
        impl_ref: ImplRef,
        receiver_ty: &Ty,
    ) -> Result<Option<(Substitution, TraitApplicability)>, D::Error> {
        let Some(header) = self.impl_header(impl_ref)? else {
            return Ok(None);
        };
        Ok(Self::impl_self_subst(&header, receiver_ty))
    }

    /// Match an already-lowered impl header without interpreting its source syntax again.
    fn impl_self_subst(
        header: &ImplHeader,
        receiver_ty: &Ty,
    ) -> Option<(Substitution, TraitApplicability)> {
        let mut subst = Substitution::new();
        let applicability = Self::match_ty(
            GenericDefRef::Impl(header.owner),
            &header.self_ty,
            receiver_ty,
            &mut subst,
        )?;
        Some((subst, applicability))
    }

    fn match_ty(
        owner: GenericDefRef,
        pattern: &Ty,
        evidence: &Ty,
        subst: &mut Substitution,
    ) -> Option<TraitApplicability> {
        if let Ty::Param(param) = pattern
            && param.owner == owner
        {
            let key = GenericParamRef::Type(*param);
            if let Some(existing) = subst.get(key) {
                return (existing.as_ty() == Some(evidence)).then_some(TraitApplicability::Yes);
            }
            subst.push(key, GenericArg::Type(Box::new(evidence.clone())));
            return Some(TraitApplicability::Yes);
        }
        if matches!(pattern, Ty::Unknown) || matches!(evidence, Ty::Unknown | Ty::InferVar { .. }) {
            return Some(TraitApplicability::Maybe);
        }

        let mut applicability = TraitApplicability::Yes;
        let matches = match (pattern, evidence) {
            (Ty::Unit, Ty::Unit)
            | (Ty::Never, Ty::Never)
            | (Ty::Primitive(_), Ty::Primitive(_)) => pattern == evidence,
            (Ty::Closure(pattern), Ty::Closure(evidence))
                if pattern.id == evidence.id && pattern.params.len() == evidence.params.len() =>
            {
                for (pattern, evidence) in pattern.params.iter().zip(&evidence.params) {
                    applicability =
                        applicability.and(Self::match_ty(owner, pattern, evidence, subst)?);
                }
                applicability =
                    applicability.and(Self::match_ty(owner, &pattern.ret, &evidence.ret, subst)?);
                true
            }
            (Ty::Tuple(pattern), Ty::Tuple(evidence)) if pattern.len() == evidence.len() => {
                for (pattern, evidence) in pattern.iter().zip(evidence) {
                    applicability =
                        applicability.and(Self::match_ty(owner, pattern, evidence, subst)?);
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
                applicability = applicability.and(Self::match_const(
                    owner,
                    *pattern_len,
                    *evidence_len,
                    subst,
                )?);
                applicability = applicability.and(Self::match_ty(owner, pattern, evidence, subst)?);
                true
            }
            (Ty::Slice(pattern), Ty::Slice(evidence)) => {
                applicability = applicability.and(Self::match_ty(owner, pattern, evidence, subst)?);
                true
            }
            (
                Ty::Reference {
                    mutability: pattern_mutability,
                    inner: pattern,
                    ..
                },
                Ty::Reference {
                    mutability: evidence_mutability,
                    inner: evidence,
                    ..
                },
            )
            | (
                Ty::RawPointer {
                    mutability: pattern_mutability,
                    inner: pattern,
                },
                Ty::RawPointer {
                    mutability: evidence_mutability,
                    inner: evidence,
                },
            ) if pattern_mutability == evidence_mutability => {
                applicability = applicability.and(Self::match_ty(owner, pattern, evidence, subst)?);
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
                        applicability.and(Self::match_ty(owner, pattern, evidence, subst)?);
                }
                applicability =
                    applicability.and(Self::match_ty(owner, pattern_ret, evidence_ret, subst)?);
                true
            }
            (Ty::Adt(pattern), Ty::Adt(evidence)) if pattern.def == evidence.def => {
                applicability = applicability.and(Self::match_args(
                    owner,
                    &pattern.args,
                    &evidence.args,
                    subst,
                )?);
                true
            }
            (Ty::FnDef(pattern), Ty::FnDef(evidence)) if pattern.def == evidence.def => {
                applicability = applicability.and(Self::match_args(
                    owner,
                    &pattern.args,
                    &evidence.args,
                    subst,
                )?);
                true
            }
            (Ty::Alias(pattern), Ty::Alias(evidence)) if pattern.same_definition(evidence) => {
                applicability = applicability.and(Self::match_args(
                    owner,
                    pattern.args(),
                    evidence.args(),
                    subst,
                )?);
                true
            }
            (_, Ty::Alias(_)) | (Ty::Alias(_), _) | (Ty::Param(_), _) => {
                applicability = TraitApplicability::Maybe;
                true
            }
            _ => false,
        };
        matches.then_some(applicability)
    }

    fn match_args(
        owner: GenericDefRef,
        patterns: &[GenericArg],
        evidence: &[GenericArg],
        subst: &mut Substitution,
    ) -> Option<TraitApplicability> {
        if patterns.len() != evidence.len() {
            return None;
        }
        let mut applicability = TraitApplicability::Yes;
        for (pattern, evidence) in patterns.iter().zip(evidence) {
            let arg_applicability = match (pattern, evidence) {
                (GenericArg::Type(pattern), GenericArg::Type(evidence)) => {
                    Self::match_ty(owner, pattern, evidence, subst)?
                }
                (GenericArg::Lifetime(pattern), GenericArg::Lifetime(evidence)) => {
                    if let Lifetime::Param(param) = pattern
                        && param.owner == owner
                    {
                        let key = GenericParamRef::Lifetime(*param);
                        if let Some(GenericArg::Lifetime(existing)) = subst.get(key) {
                            if existing != evidence
                                && !matches!(existing, Lifetime::Erased)
                                && !matches!(evidence, Lifetime::Erased)
                            {
                                return None;
                            }
                        } else {
                            subst.push(key, GenericArg::Lifetime(*evidence));
                        }
                        TraitApplicability::Yes
                    } else {
                        match (pattern, evidence) {
                            (Lifetime::Static, Lifetime::Static) => TraitApplicability::Yes,
                            (Lifetime::Static, _) | (_, Lifetime::Static) => return None,
                            (Lifetime::Param(_) | Lifetime::Erased, _) => TraitApplicability::Yes,
                        }
                    }
                }
                (GenericArg::Const(pattern), GenericArg::Const(evidence)) => {
                    Self::match_const(owner, *pattern, *evidence, subst)?
                }
                _ => return None,
            };
            applicability = applicability.and(arg_applicability);
        }
        Some(applicability)
    }

    fn match_const(
        owner: GenericDefRef,
        pattern: ConstValue,
        evidence: ConstValue,
        subst: &mut Substitution,
    ) -> Option<TraitApplicability> {
        if let ConstValue::Param(param) = pattern
            && param.owner == owner
        {
            let key = GenericParamRef::Const(param);
            if let Some(GenericArg::Const(existing)) = subst.get(key) {
                return match (*existing, evidence) {
                    (ConstValue::Scalar(lhs), ConstValue::Scalar(rhs)) => {
                        (lhs == rhs).then_some(TraitApplicability::Yes)
                    }
                    (ConstValue::Unknown, _) | (_, ConstValue::Unknown) => {
                        Some(TraitApplicability::Maybe)
                    }
                    (ConstValue::Param(_), _) | (_, ConstValue::Param(_)) => {
                        Some(TraitApplicability::Yes)
                    }
                };
            }
            subst.push(key, GenericArg::Const(evidence));
            return Some(TraitApplicability::Yes);
        }

        match (pattern, evidence) {
            (ConstValue::Scalar(lhs), ConstValue::Scalar(rhs)) => {
                (lhs == rhs).then_some(TraitApplicability::Yes)
            }
            (ConstValue::Unknown, _) | (_, ConstValue::Unknown) => Some(TraitApplicability::Maybe),
            (ConstValue::Param(_), _) | (_, ConstValue::Param(_)) => Some(TraitApplicability::Yes),
        }
    }
}
