//! Dot-completion assembly for member access sites.

use rg_body_ir::{
    BodyAutoderef, BodyAutoderefMode, BodyLocalNominalTy, BodyNominalTy, DotCompletionSite,
    ResolvedFieldRef, ResolvedFunctionRef,
};

use crate::{
    Analysis,
    model::{
        CompletionApplicability, CompletionEdit, CompletionItem, CompletionKind, CompletionTarget,
    },
};

use super::{
    CompletionQuery,
    completion_sort::CompletionSortPolicy,
    field::FieldCompletionRenderer,
    function::{FunctionCallCompletion, FunctionCompletionRenderer, FunctionCompletionRequest},
};

pub(super) struct DotCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> DotCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Collects member completions for a dot site like `user.na$0`.
    pub(super) fn completions(
        &self,
        site: DotCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let Some(receiver_ty) = self.analysis.body_ir.receiver_ty(site)? else {
            return Ok(Vec::new());
        };

        let edit = CompletionEdit {
            replace: site.member_prefix_span,
        };
        let autoderef = BodyAutoderef::new(&self.analysis.def_map, &self.analysis.semantic_ir);
        let mut completions = Vec::new();
        for candidate in autoderef.candidates(BodyAutoderefMode::FieldLookup, &receiver_ty) {
            let candidate = candidate?;
            for ty in candidate.ty().as_local_nominals() {
                self.push_local_type_field_completions(ty, edit, &mut completions)?;
            }
            for ty in candidate.ty().as_nominals() {
                self.push_type_field_completions(ty, edit, &mut completions)?;
            }
        }
        for candidate in autoderef.candidates(BodyAutoderefMode::MethodReceiver, &receiver_ty) {
            let candidate = candidate?;
            for ty in candidate.ty().as_local_nominals() {
                self.push_local_type_method_completions(ty, edit, &mut completions)?;
            }
            for ty in candidate.ty().as_nominals() {
                self.push_type_method_completions(ty, edit, &mut completions)?;
            }
        }
        // Keep snapshot output and editor ordering stable across equivalent resolution paths.
        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }

    /// Adds field candidates for a resolved semantic receiver type, such as `User` in `user.$0`.
    fn push_type_field_completions(
        &self,
        ty: &BodyNominalTy,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        for field in self.analysis.semantic_ir.fields_for_type(ty.def)? {
            self.push_field_completion(ResolvedFieldRef::Semantic(field), edit, completions)?;
        }
        Ok(())
    }

    /// Adds method candidates for a resolved semantic receiver type, such as `User` in `user.$0`.
    fn push_type_method_completions(
        &self,
        ty: &BodyNominalTy,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        // Trait candidates carry applicability because this project intentionally avoids full
        // solving.
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

            self.push_function_completion(
                ResolvedFunctionRef::Semantic(function),
                CompletionKind::InherentMethod,
                CompletionApplicability::Known,
                edit,
                completions,
            )?;
        }

        for (function, applicability) in self
            .analysis
            .body_ir
            .semantic_trait_function_candidates_for_receiver(
                &self.analysis.def_map,
                &self.analysis.semantic_ir,
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

    /// Adds field candidates for a body-local receiver type.
    fn push_local_type_field_completions(
        &self,
        ty: &BodyLocalNominalTy,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        // Body-local structs are visible only through Body IR, so their member
        // data comes from body-local lowering rather than Semantic IR.
        for field in self.analysis.body_ir.fields_for_local_type(ty.item)? {
            self.push_field_completion(ResolvedFieldRef::BodyLocal(field), edit, completions)?;
        }
        Ok(())
    }

    /// Adds method candidates for a body-local receiver type.
    fn push_local_type_method_completions(
        &self,
        ty: &BodyLocalNominalTy,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
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
        let Some(completion) =
            FieldCompletionRenderer::new(self.analysis).completion(field, edit)?
        else {
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
        let renderer = FunctionCompletionRenderer::new(self.analysis, self.query);
        let target = CompletionTarget::Function(function);
        let Some(completion) = renderer.completion(FunctionCompletionRequest {
            function,
            label_override: None,
            kind,
            applicability,
            edit,
            call_completion: FunctionCallCompletion::MethodCall,
            sort_policy: CompletionSortPolicy::General,
            sort_priority: None,
        })?
        else {
            return Ok(());
        };
        if !completion.has_self_receiver {
            return Ok(());
        }
        if completions
            .iter()
            .any(|completion| completion.target == target)
        {
            return Ok(());
        }

        completions.push(completion.item);
        Ok(())
    }
}
