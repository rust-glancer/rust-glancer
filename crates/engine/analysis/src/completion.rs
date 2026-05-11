//! Dot-completion assembly for known receiver types.
//!
//! Body IR finds the receiver before the dot and classifies its type. This layer collects fields
//! and methods from semantic and body-local item stores into stable completion rows.

use rg_body_ir::{BodyLocalNominalTy, BodyNominalTy, ResolvedFieldRef, ResolvedFunctionRef};
use rg_def_map::TargetRef;
use rg_parse::FileId;
use rg_semantic_ir::TraitApplicability;

use super::{
    Analysis,
    data::{CompletionApplicability, CompletionItem, CompletionKind, CompletionTarget},
};

pub(super) struct CompletionResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> CompletionResolver<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(super) fn completions_at_dot(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let Some(receiver) = self.0.body_ir.receiver_at_dot(target, file_id, offset)? else {
            return Ok(Vec::new());
        };
        let Some(receiver_ty) = self.0.body_ir.receiver_ty(receiver)? else {
            return Ok(Vec::new());
        };

        let mut completions = Vec::new();
        for ty in receiver_ty.local_nominals() {
            self.push_local_type_completions(ty, &mut completions)?;
        }
        for ty in receiver_ty.nominal_tys() {
            self.push_type_completions(ty, &mut completions)?;
        }
        // Keep snapshot output and editor ordering stable across equivalent resolution paths.
        completions.sort_by(|left, right| {
            left.label
                .cmp(&right.label)
                .then(left.kind.cmp(&right.kind))
                .then(left.applicability.cmp(&right.applicability))
        });
        Ok(completions)
    }

    fn push_type_completions(
        &self,
        ty: &BodyNominalTy,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        // Semantic nominal types can offer fields, inherent methods, and trait methods. Trait
        // candidates carry applicability because this project intentionally avoids full solving.
        for field in self.0.semantic_ir.fields_for_type(ty.def)? {
            self.push_field_completion(ResolvedFieldRef::Semantic(field), completions)?;
        }

        for function in self.0.semantic_ir.inherent_functions_for_type(ty.def)? {
            if !self.0.body_ir.semantic_function_applies_to_receiver(
                &self.0.def_map,
                &self.0.semantic_ir,
                function,
                ty,
            )? {
                continue;
            }

            self.push_function_completion(
                ResolvedFunctionRef::Semantic(function),
                CompletionKind::InherentMethod,
                CompletionApplicability::Known,
                completions,
            )?;
        }

        for (function, applicability) in self
            .0
            .body_ir
            .semantic_trait_function_candidates_for_receiver(
                &self.0.def_map,
                &self.0.semantic_ir,
                ty,
            )?
        {
            self.push_function_completion(
                ResolvedFunctionRef::Semantic(function),
                CompletionKind::TraitMethod,
                CompletionApplicability::from(applicability),
                completions,
            )?;
        }
        Ok(())
    }

    fn push_local_type_completions(
        &self,
        ty: &BodyLocalNominalTy,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        // Body-local structs are visible only through Body IR, and currently only support
        // inherent methods from body-local impls.
        for field in self.0.body_ir.fields_for_local_type(ty.item)? {
            self.push_field_completion(ResolvedFieldRef::BodyLocal(field), completions)?;
        }

        for function in self.0.body_ir.inherent_functions_for_local_type(ty.item)? {
            if !self.0.body_ir.local_function_applies_to_receiver(
                &self.0.def_map,
                &self.0.semantic_ir,
                function,
                ty,
            )? {
                continue;
            }

            self.push_function_completion(
                ResolvedFunctionRef::BodyLocal(function),
                CompletionKind::InherentMethod,
                CompletionApplicability::Known,
                completions,
            )?;
        }
        Ok(())
    }

    fn push_field_completion(
        &self,
        field: ResolvedFieldRef,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        let Some(label) = self.field_completion_label(field)? else {
            return Ok(());
        };
        let target = CompletionTarget::Field(field);
        if completions
            .iter()
            .any(|completion| completion.target == target)
        {
            return Ok(());
        }

        completions.push(CompletionItem {
            label,
            kind: CompletionKind::Field,
            target,
            applicability: CompletionApplicability::Known,
        });
        Ok(())
    }

    fn field_completion_label(&self, field: ResolvedFieldRef) -> anyhow::Result<Option<String>> {
        match field {
            ResolvedFieldRef::Semantic(field) => Ok(self
                .0
                .semantic_ir
                .field_data(field)?
                .and_then(|data| data.field.key.as_ref().map(ToString::to_string))),
            ResolvedFieldRef::BodyLocal(field) => Ok(self
                .0
                .body_ir
                .local_field_data(field)?
                .and_then(|data| data.field.key.as_ref().map(ToString::to_string))),
        }
    }

    fn push_function_completion(
        &self,
        function: ResolvedFunctionRef,
        kind: CompletionKind,
        applicability: CompletionApplicability,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        let Some(name) = self.function_completion_name(function)? else {
            return Ok(());
        };
        if completions
            .iter()
            .any(|completion| completion.target == CompletionTarget::Function(function))
        {
            return Ok(());
        }

        completions.push(CompletionItem {
            label: name,
            kind,
            target: CompletionTarget::Function(function),
            applicability,
        });
        Ok(())
    }

    fn function_completion_name(
        &self,
        function: ResolvedFunctionRef,
    ) -> anyhow::Result<Option<String>> {
        match function {
            ResolvedFunctionRef::Semantic(function) => {
                let Some(data) = self.0.semantic_ir.function_data(function)? else {
                    return Ok(None);
                };
                Ok(data.has_self_receiver().then(|| data.name.to_string()))
            }
            ResolvedFunctionRef::BodyLocal(function) => {
                let Some(data) = self.0.body_ir.local_function_data(function)? else {
                    return Ok(None);
                };
                Ok(data.has_self_receiver().then(|| data.name.to_string()))
            }
        }
    }
}

impl From<TraitApplicability> for CompletionApplicability {
    fn from(applicability: TraitApplicability) -> Self {
        match applicability {
            TraitApplicability::Yes => Self::Known,
            TraitApplicability::Maybe | TraitApplicability::No => Self::Maybe,
        }
    }
}
