//! Dot-completion assembly for member access sites.

use rg_body_ir::{
    BodyAutoderef, BodyAutoderefMode, DotCompletionSite, ResolvedFieldRef, ResolvedFunctionRef,
};

use crate::{
    Analysis,
    api::query::member::{
        MemberLookup, MemberMethodCandidate, MemberMethodOrigin, MemberReceiverTy,
    },
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
        let members = MemberLookup::new(self.analysis);
        let mut completions = Vec::new();
        for candidate in autoderef.candidates(BodyAutoderefMode::FieldLookup, receiver_ty) {
            let candidate = candidate?;
            for ty in MemberReceiverTy::in_body_ty(candidate.ty()) {
                for field in members.field_candidates(ty)? {
                    self.push_field_completion(field, edit, &mut completions)?;
                }
            }
        }
        for candidate in autoderef.candidates(BodyAutoderefMode::MethodReceiver, receiver_ty) {
            let candidate = candidate?;
            for ty in MemberReceiverTy::in_body_ty(candidate.ty()) {
                for method in members.method_candidates(ty)? {
                    self.push_method_completion(method, edit, &mut completions)?;
                }
            }
        }
        // Keep snapshot output and editor ordering stable across equivalent resolution paths.
        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }

    fn push_method_completion(
        &self,
        method: MemberMethodCandidate,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        match method.origin {
            MemberMethodOrigin::Inherent => self.push_function_completion(
                method.function,
                CompletionKind::InherentMethod,
                CompletionApplicability::Known,
                edit,
                completions,
            ),
            MemberMethodOrigin::Trait { applicability } => self.push_function_completion(
                method.function,
                CompletionKind::TraitMethod,
                CompletionApplicability::from(applicability),
                edit,
                completions,
            ),
        }
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
