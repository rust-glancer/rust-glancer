//! Completion assembly for source positions.

use rg_body_ir::{
    BodyIrReadTxn, BodyLocalNominalTy, BodyNominalTy, DotCompletionSite, ResolvedFieldRef,
    ResolvedFunctionRef,
};
use rg_def_map::TargetRef;
use rg_parse::FileId;
use rg_semantic_ir::{Documentation, TraitApplicability};

use crate::{
    Analysis,
    api::render::signature::SignatureRenderer,
    model::{
        CompletionApplicability, CompletionEdit, CompletionItem, CompletionKind, CompletionTarget,
    },
};

pub(crate) struct CompletionResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> CompletionResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn completions_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let Some(context) = CompletionContext::at(&self.0.body_ir, target, file_id, offset)? else {
            return Ok(Vec::new());
        };

        match context {
            CompletionContext::DotCompletionSite(site) => {
                self.dot_completion_site_completions(site)
            }
        }
    }

    fn dot_completion_site_completions(
        &self,
        site: DotCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let Some(receiver_ty) = self.0.body_ir.receiver_ty(site)? else {
            return Ok(Vec::new());
        };

        let edit = CompletionEdit {
            replace: site.member_prefix_span,
        };
        let mut completions = Vec::new();
        for ty in receiver_ty.local_nominals() {
            self.push_local_type_completions(ty, edit, &mut completions)?;
        }
        for ty in receiver_ty.nominal_tys() {
            self.push_type_completions(ty, edit, &mut completions)?;
        }
        // Keep snapshot output and editor ordering stable across equivalent resolution paths.
        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }

    fn push_type_completions(
        &self,
        ty: &BodyNominalTy,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        // Semantic nominal types can offer fields, inherent methods, and trait methods. Trait
        // candidates carry applicability because this project intentionally avoids full solving.
        for field in self.0.semantic_ir.fields_for_type(ty.def)? {
            self.push_field_completion(ResolvedFieldRef::Semantic(field), edit, completions)?;
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
                edit,
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
                edit,
                completions,
            )?;
        }
        Ok(())
    }

    fn push_local_type_completions(
        &self,
        ty: &BodyLocalNominalTy,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        // Body-local structs are visible only through Body IR, and currently only support
        // inherent methods from body-local impls.
        for field in self.0.body_ir.fields_for_local_type(ty.item)? {
            self.push_field_completion(ResolvedFieldRef::BodyLocal(field), edit, completions)?;
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
                edit,
                completions,
            )?;
        }
        Ok(())
    }

    fn push_field_completion(
        &self,
        field: ResolvedFieldRef,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        let Some(metadata) = self.field_completion_metadata(field)? else {
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
            label: metadata.label.clone(),
            kind: CompletionKind::Field,
            target,
            applicability: CompletionApplicability::Known,
            detail: metadata.detail,
            documentation: metadata.documentation,
            sort_text: self.completion_sort_text(
                &metadata.label,
                CompletionKind::Field,
                CompletionApplicability::Known,
                target,
            ),
            edit: Some(edit),
        });
        Ok(())
    }

    fn field_completion_metadata(
        &self,
        field: ResolvedFieldRef,
    ) -> anyhow::Result<Option<CompletionMetadata>> {
        let renderer = SignatureRenderer::new(self.0);
        match field {
            ResolvedFieldRef::Semantic(field) => {
                let Some(data) = self.0.semantic_ir.field_data(field)? else {
                    return Ok(None);
                };
                let Some(label) = data.field.key.as_ref().map(ToString::to_string) else {
                    return Ok(None);
                };
                Ok(Some(CompletionMetadata {
                    label,
                    detail: renderer.field_signature(data),
                    documentation: docs_text(data.field.docs.as_ref()),
                }))
            }
            ResolvedFieldRef::BodyLocal(field) => {
                let Some(data) = self.0.body_ir.local_field_data(field)? else {
                    return Ok(None);
                };
                let Some(label) = data.field.key.as_ref().map(ToString::to_string) else {
                    return Ok(None);
                };
                Ok(Some(CompletionMetadata {
                    label,
                    detail: renderer.local_field_signature(data),
                    documentation: docs_text(data.field.docs.as_ref()),
                }))
            }
        }
    }

    fn push_function_completion(
        &self,
        function: ResolvedFunctionRef,
        kind: CompletionKind,
        applicability: CompletionApplicability,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        let Some(metadata) = self.function_completion_metadata(function)? else {
            return Ok(());
        };
        if completions
            .iter()
            .any(|completion| completion.target == CompletionTarget::Function(function))
        {
            return Ok(());
        }

        completions.push(CompletionItem {
            label: metadata.label.clone(),
            kind,
            target: CompletionTarget::Function(function),
            applicability,
            detail: metadata.detail,
            documentation: metadata.documentation,
            sort_text: self.completion_sort_text(
                &metadata.label,
                kind,
                applicability,
                CompletionTarget::Function(function),
            ),
            edit: Some(edit),
        });
        Ok(())
    }

    fn function_completion_metadata(
        &self,
        function: ResolvedFunctionRef,
    ) -> anyhow::Result<Option<CompletionMetadata>> {
        let renderer = SignatureRenderer::new(self.0);
        match function {
            ResolvedFunctionRef::Semantic(function) => {
                let Some(data) = self.0.semantic_ir.function_data(function)? else {
                    return Ok(None);
                };
                if !data.has_self_receiver() {
                    return Ok(None);
                }
                Ok(Some(CompletionMetadata {
                    label: data.name.to_string(),
                    detail: Some(renderer.function_signature(data)),
                    documentation: docs_text(data.docs.as_ref()),
                }))
            }
            ResolvedFunctionRef::BodyLocal(function) => {
                let Some(data) = self.0.body_ir.local_function_data(function)? else {
                    return Ok(None);
                };
                if !data.has_self_receiver() {
                    return Ok(None);
                }
                Ok(Some(CompletionMetadata {
                    label: data.name.to_string(),
                    detail: Some(renderer.local_function_signature(data)),
                    documentation: docs_text(data.docs.as_ref()),
                }))
            }
        }
    }

    fn completion_sort_text(
        &self,
        label: &str,
        kind: CompletionKind,
        applicability: CompletionApplicability,
        target: CompletionTarget,
    ) -> String {
        format!(
            "{label}|{:02}|{:02}|{target:?}",
            Self::completion_kind_rank(kind),
            Self::completion_applicability_rank(applicability),
        )
    }

    fn completion_kind_rank(kind: CompletionKind) -> u8 {
        match kind {
            CompletionKind::Field => 0,
            CompletionKind::InherentMethod => 1,
            CompletionKind::TraitMethod => 2,
        }
    }

    fn completion_applicability_rank(applicability: CompletionApplicability) -> u8 {
        match applicability {
            CompletionApplicability::Known => 0,
            CompletionApplicability::Maybe => 1,
        }
    }
}

struct CompletionMetadata {
    label: String,
    detail: Option<String>,
    documentation: Option<String>,
}

fn docs_text(docs: Option<&Documentation>) -> Option<String> {
    docs.map(|docs| docs.as_str().to_string())
}

enum CompletionContext {
    DotCompletionSite(DotCompletionSite),
}

impl CompletionContext {
    fn at(
        body_ir: &BodyIrReadTxn<'_>,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<Self>> {
        Ok(body_ir
            .dot_completion_site(target, file_id, offset)?
            .map(Self::DotCompletionSite))
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
