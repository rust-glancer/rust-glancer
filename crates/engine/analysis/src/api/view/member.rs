//! Composite member view over semantic and body-local nominal types.

use rg_body_ir::{
    BodyFieldData, BodyFieldRef, BodyFunctionData, BodyFunctionRef, BodyItemRef,
    BodyLocalNominalTy, BodyNominalTy, BodyTy, BodyTyExt, ResolvedFieldRef, ResolvedFunctionRef,
};
use rg_def_map::LocalDefRef;
use rg_semantic_ir::{
    Documentation, FieldData, FieldKey, FieldRef, FunctionData, FunctionRef, ItemOwner, ParamItem,
    SemanticItemRef, TraitApplicability, TypeDefRef,
};

use crate::{
    api::{Analysis, render::path::PathRenderer},
    model::SymbolKind,
};

use super::declaration::Declaration;

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

    fn owner(self) -> MemberOwnerRef {
        match self {
            Self::Semantic(ty) => MemberOwnerRef::Semantic(ty.def),
            Self::BodyLocal(ty) => MemberOwnerRef::BodyLocal(ty.item),
        }
    }
}

/// Reference to a declaration owner whose fields can be enumerated without receiver generic args.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MemberOwnerRef {
    Semantic(TypeDefRef),
    BodyLocal(BodyItemRef),
}

/// Borrowed data for one resolved field, independent from the storage layer it came from.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MemberField<'a> {
    Semantic {
        field: FieldRef,
        data: FieldData<'a>,
    },
    BodyLocal {
        field: BodyFieldRef,
        data: BodyFieldData<'a>,
    },
}

impl<'a> MemberField<'a> {
    pub(crate) fn field_ref(&self) -> ResolvedFieldRef {
        match self {
            Self::Semantic { field, .. } => ResolvedFieldRef::Semantic(*field),
            Self::BodyLocal { field, .. } => ResolvedFieldRef::BodyLocal(*field),
        }
    }

    pub(crate) fn key(&self) -> Option<&'a FieldKey> {
        match self {
            Self::Semantic { data, .. } => data.field.key.as_ref(),
            Self::BodyLocal { data, .. } => data.field.key.as_ref(),
        }
    }

    pub(crate) fn display_path(
        &self,
        paths: &PathRenderer<'_, '_>,
    ) -> anyhow::Result<Option<String>> {
        match self {
            Self::Semantic { field, .. } => paths.type_def_path(field.owner),
            Self::BodyLocal { .. } => Ok(None),
        }
    }

    pub(crate) fn declaration(&self) -> Option<Declaration> {
        let key = self.key()?;
        Some(match self {
            Self::Semantic { field, data } => Declaration::new(
                field.owner.target,
                SymbolKind::Field,
                key.declaration_label(),
                data.file_id,
                data.field.span,
                data.field.span,
            ),
            Self::BodyLocal { field, data } => Declaration::new(
                field.item.body.target,
                SymbolKind::Field,
                key.declaration_label(),
                data.item.source.file_id,
                data.field.span,
                data.field.span,
            ),
        })
    }

    pub(crate) fn docs_text(&self) -> Option<String> {
        self.docs().map(Documentation::text)
    }

    fn docs(&self) -> Option<&'a Documentation> {
        match self {
            Self::Semantic { data, .. } => data.field.docs.as_ref(),
            Self::BodyLocal { data, .. } => data.field.docs.as_ref(),
        }
    }
}

/// Borrowed data for one resolved function, independent from the storage layer it came from.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MemberFunction<'a> {
    Semantic {
        function: FunctionRef,
        data: &'a FunctionData,
    },
    BodyLocal {
        function: BodyFunctionRef,
        data: &'a BodyFunctionData,
    },
}

impl<'a> MemberFunction<'a> {
    pub(crate) fn function_ref(&self) -> ResolvedFunctionRef {
        match self {
            Self::Semantic { function, .. } => ResolvedFunctionRef::Semantic(*function),
            Self::BodyLocal { function, .. } => ResolvedFunctionRef::BodyLocal(*function),
        }
    }

    pub(crate) fn name(&self) -> &'a str {
        match self {
            Self::Semantic { data, .. } => data.name.as_str(),
            Self::BodyLocal { data, .. } => data.name.as_str(),
        }
    }

    pub(crate) fn params(&self) -> &'a [ParamItem] {
        match self {
            Self::Semantic { data, .. } => data.signature.params(),
            Self::BodyLocal { data, .. } => &data.declaration.params,
        }
    }

    pub(crate) fn display_path(
        &self,
        paths: &PathRenderer<'_, '_>,
    ) -> anyhow::Result<Option<String>> {
        match self {
            Self::Semantic { function, .. } => paths.function_path(*function),
            Self::BodyLocal { .. } => Ok(None),
        }
    }

    pub(crate) fn symbol_kind(&self) -> SymbolKind {
        match self {
            Self::Semantic { data, .. } => match data.owner {
                ItemOwner::Module(_) => SymbolKind::Function,
                ItemOwner::Trait(_) | ItemOwner::Impl(_) => SymbolKind::Method,
            },
            Self::BodyLocal { data, .. } => SymbolKind::from_body_function_owner(data.owner),
        }
    }

    pub(crate) fn declaration(&self) -> Declaration {
        match self {
            Self::Semantic { function, data } => Declaration::new(
                function.target,
                self.symbol_kind(),
                data.name.to_string(),
                data.source.file_id,
                data.span,
                data.name_span.unwrap_or(data.span),
            ),
            Self::BodyLocal { function, data } => Declaration::new(
                function.body.target,
                self.symbol_kind(),
                data.name.to_string(),
                data.source.file_id,
                data.source.span,
                data.name_source.span,
            ),
        }
    }

    pub(crate) fn docs_text(&self) -> Option<String> {
        self.docs().map(Documentation::text)
    }

    pub(crate) fn has_self_receiver(&self) -> bool {
        match self {
            Self::Semantic { data, .. } => data.has_self_receiver(),
            Self::BodyLocal { data, .. } => data.has_self_receiver(),
        }
    }

    fn docs(&self) -> Option<&'a Documentation> {
        match self {
            Self::Semantic { data, .. } => data.docs.as_ref(),
            Self::BodyLocal { data, .. } => data.docs.as_ref(),
        }
    }
}

/// One method candidate with enough origin information for UI ranking and labels.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MemberMethodCandidate<'a> {
    pub(crate) function: MemberFunction<'a>,
    pub(crate) origin: MemberMethodOrigin,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum MemberMethodOrigin {
    Inherent,
    Trait { applicability: TraitApplicability },
}

pub(crate) struct MemberView<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> MemberView<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn field_candidates<'view>(
        &'view self,
        receiver_ty: MemberReceiverTy<'_>,
    ) -> anyhow::Result<Vec<MemberField<'view>>> {
        self.field_candidates_for_owner(receiver_ty.owner())
    }

    pub(crate) fn field_candidates_for_owner<'view>(
        &'view self,
        owner: MemberOwnerRef,
    ) -> anyhow::Result<Vec<MemberField<'view>>> {
        let field_refs: Vec<_> = match owner {
            MemberOwnerRef::Semantic(ty) => self
                .analysis
                .semantic_ir
                .fields_for_type(ty)?
                .into_iter()
                .map(ResolvedFieldRef::Semantic)
                .collect(),
            MemberOwnerRef::BodyLocal(item) => self
                .analysis
                .body_ir
                .fields_for_local_type(item)?
                .into_iter()
                .map(ResolvedFieldRef::BodyLocal)
                .collect(),
        };

        let mut fields = Vec::new();
        for field_ref in field_refs {
            let Some(field) = self.field(field_ref)? else {
                continue;
            };
            fields.push(field);
        }

        Ok(fields)
    }

    pub(crate) fn field(&self, field: ResolvedFieldRef) -> anyhow::Result<Option<MemberField<'_>>> {
        match field {
            ResolvedFieldRef::Semantic(field) => Ok(self
                .analysis
                .semantic_ir
                .field_data(field)?
                .map(|data| MemberField::Semantic { field, data })),
            ResolvedFieldRef::BodyLocal(field) => Ok(self
                .analysis
                .body_ir
                .local_field_data(field)?
                .map(|data| MemberField::BodyLocal { field, data })),
        }
    }

    pub(crate) fn function(
        &self,
        function: ResolvedFunctionRef,
    ) -> anyhow::Result<Option<MemberFunction<'_>>> {
        match function {
            ResolvedFunctionRef::Semantic(function) => Ok(self
                .analysis
                .semantic_ir
                .function_data(function)?
                .map(|data| MemberFunction::Semantic { function, data })),
            ResolvedFunctionRef::BodyLocal(function) => Ok(self
                .analysis
                .body_ir
                .local_function_data(function)?
                .map(|data| MemberFunction::BodyLocal { function, data })),
        }
    }

    pub(crate) fn function_for_local_def(
        &self,
        local_def: LocalDefRef,
    ) -> anyhow::Result<Option<MemberFunction<'_>>> {
        let Some(SemanticItemRef::Function(function)) = self
            .analysis
            .semantic_ir
            .semantic_item_for_local_def(local_def)?
        else {
            return Ok(None);
        };

        self.function(ResolvedFunctionRef::Semantic(function))
    }

    pub(crate) fn method_candidates<'view>(
        &'view self,
        receiver_ty: MemberReceiverTy<'_>,
    ) -> anyhow::Result<Vec<MemberMethodCandidate<'view>>> {
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

                    let Some(function) = self.function(ResolvedFunctionRef::Semantic(function))?
                    else {
                        continue;
                    };
                    candidates.push(MemberMethodCandidate {
                        function,
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
                    let Some(function) = self.function(ResolvedFunctionRef::Semantic(function))?
                    else {
                        continue;
                    };
                    candidates.push(MemberMethodCandidate {
                        function,
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

                    let Some(function) = self.function(ResolvedFunctionRef::BodyLocal(function))?
                    else {
                        continue;
                    };
                    candidates.push(MemberMethodCandidate {
                        function,
                        origin: MemberMethodOrigin::Inherent,
                    });
                }
            }
        }

        Ok(candidates)
    }
}
