//! Unified member lookup over semantic and body-local nominal types.

use rg_body_ir::{
    BodyItemRef, BodyLocalNominalTy, BodyNominalTy, BodyTy, ResolvedFieldRef, ResolvedFunctionRef,
};
use rg_semantic_ir::{TraitApplicability, TypeDefRef};

use crate::api::Analysis;

/// A nominal receiver type whose declarations may live in either Semantic IR or Body IR.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MemberReceiverTy<'a> {
    Semantic(&'a BodyNominalTy),
    BodyLocal(&'a BodyLocalNominalTy),
}

impl<'a> MemberReceiverTy<'a> {
    pub(crate) fn in_body_ty(ty: &'a BodyTy) -> impl Iterator<Item = Self> + 'a {
        ty.as_local_nominals()
            .iter()
            .map(Self::BodyLocal)
            .chain(ty.as_nominals().iter().map(Self::Semantic))
    }

    fn owner(self) -> MemberOwner {
        match self {
            Self::Semantic(ty) => MemberOwner::Semantic(ty.def),
            Self::BodyLocal(ty) => MemberOwner::BodyLocal(ty.item),
        }
    }
}

/// A declaration owner whose fields can be enumerated without receiver-specific generic args.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MemberOwner {
    Semantic(TypeDefRef),
    BodyLocal(BodyItemRef),
}

/// One method candidate with enough origin information for UI ranking and labels.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MemberMethodCandidate {
    pub(crate) function: ResolvedFunctionRef,
    pub(crate) origin: MemberMethodOrigin,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum MemberMethodOrigin {
    Inherent,
    Trait { applicability: TraitApplicability },
}

pub(crate) struct MemberLookup<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> MemberLookup<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn field_candidates(
        &self,
        receiver_ty: MemberReceiverTy<'_>,
    ) -> anyhow::Result<Vec<ResolvedFieldRef>> {
        self.field_candidates_for_owner(receiver_ty.owner())
    }

    pub(crate) fn field_candidates_for_owner(
        &self,
        owner: MemberOwner,
    ) -> anyhow::Result<Vec<ResolvedFieldRef>> {
        match owner {
            MemberOwner::Semantic(ty) => Ok(self
                .analysis
                .semantic_ir
                .fields_for_type(ty)?
                .into_iter()
                .map(ResolvedFieldRef::Semantic)
                .collect()),
            MemberOwner::BodyLocal(item) => Ok(self
                .analysis
                .body_ir
                .fields_for_local_type(item)?
                .into_iter()
                .map(ResolvedFieldRef::BodyLocal)
                .collect()),
        }
    }

    pub(crate) fn method_candidates(
        &self,
        receiver_ty: MemberReceiverTy<'_>,
    ) -> anyhow::Result<Vec<MemberMethodCandidate>> {
        let mut candidates = Vec::new();

        match receiver_ty {
            MemberReceiverTy::Semantic(ty) => {
                for function in self
                    .analysis
                    .semantic_ir
                    .inherent_functions_for_type(ty.def)?
                {
                    if !self
                        .analysis
                        .body_ir
                        .semantic_function_applies_to_receiver(
                            &self.analysis.def_map,
                            &self.analysis.semantic_ir,
                            function,
                            ty,
                        )?
                    {
                        continue;
                    }

                    candidates.push(MemberMethodCandidate {
                        function: ResolvedFunctionRef::Semantic(function),
                        origin: MemberMethodOrigin::Inherent,
                    });
                }

                // Trait candidates carry applicability because this project intentionally avoids
                // full solving, but still wants useful editor suggestions for likely matches.
                for (function, applicability) in self
                    .analysis
                    .body_ir
                    .semantic_trait_function_candidates_for_receiver(
                        &self.analysis.def_map,
                        &self.analysis.semantic_ir,
                        ty,
                    )?
                {
                    candidates.push(MemberMethodCandidate {
                        function: ResolvedFunctionRef::Semantic(function),
                        origin: MemberMethodOrigin::Trait { applicability },
                    });
                }
            }
            MemberReceiverTy::BodyLocal(ty) => {
                for function in self
                    .analysis
                    .body_ir
                    .inherent_functions_for_local_type(ty.item)?
                {
                    if !self.analysis.body_ir.local_function_applies_to_receiver(
                        &self.analysis.def_map,
                        &self.analysis.semantic_ir,
                        function,
                        ty,
                    )? {
                        continue;
                    }

                    candidates.push(MemberMethodCandidate {
                        function: ResolvedFunctionRef::BodyLocal(function),
                        origin: MemberMethodOrigin::Inherent,
                    });
                }
            }
        }

        Ok(candidates)
    }
}
