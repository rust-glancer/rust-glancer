//! Dot-completion assembly for member access sites.

use rg_body_ir::{
    BodyLocalNominalTy, BodyNominalTy, DotCompletionSite, ResolvedFieldRef, ResolvedFunctionRef,
};
use rg_semantic_ir::Documentation;

use crate::{
    Analysis,
    api::render::signature::SignatureRenderer,
    model::{
        CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem,
        CompletionKind, CompletionTarget,
    },
};

use super::{
    CompletionMetadata, completion_sort::CompletionSortPolicy, field::FieldCompletionRenderer,
};

pub(super) struct DotCompletionResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> DotCompletionResolver<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    /// Collects member completions for a dot site like `user.na$0`.
    pub(super) fn completions(
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

    /// Adds field and method candidates for a resolved receiver type, such as
    /// `User` in `user.$0`.
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

    /// Adds member candidates for a body-local receiver type, such as a struct
    /// declared inside the same function as `local.$0`.
    fn push_local_type_completions(
        &self,
        ty: &BodyLocalNominalTy,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        // Body-local structs are visible only through Body IR, so their member
        // data comes from body-local lowering rather than Semantic IR.
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
        let Some(completion) = FieldCompletionRenderer::new(self.0).completion(field, edit)? else {
            return Ok(());
        };
        if completions
            .iter()
            .any(|existing| existing.target == completion.item.target)
        {
            return Ok(());
        }

        completions.push(completion.item);
        Ok(())
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
            sort_text: CompletionSortPolicy::General.sort_text(
                None,
                &metadata.label,
                kind,
                applicability,
                CompletionTarget::Function(function),
            ),
            insert_text: CompletionInsertText::Plain,
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
                    documentation: data.docs.as_ref().map(Documentation::text),
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
                    documentation: data.docs.as_ref().map(Documentation::text),
                }))
            }
        }
    }
}
