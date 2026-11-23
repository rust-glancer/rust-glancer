//! Fields, methods, and postfix transforms after a semantically indexed dot receiver.
//!
//! Field and method rows come from the receiver type and keep declaration identity. Postfix rows
//! reuse the same receiver span and inferred type, but remain synthetic whole-expression
//! transformations. This module combines the three families and removes duplicate member targets.

use anyhow::Context as _;
use rg_ir_model::PrimitiveTy;
use rg_ir_view::{
    display::syntax::SyntaxRenderer,
    member::{MemberMethodOrigin, MemberView},
    ty::locals::BodyView,
};

use super::postfix::PostfixCompletionResolver;
use crate::{
    Analysis,
    model::{
        CompletionApplicability, CompletionEdit, CompletionImport, CompletionItem, CompletionKind,
    },
    query::{
        completion::{
            CompletionContext,
            relevance::{CompletionSortPolicy, CompletionSortPriority},
            render::{
                CallCompletionKind, FieldCompletionRenderer, FunctionCompletionRenderer,
                FunctionCompletionRequest,
            },
            site::DotCompletionSite,
            syntax::CompletionSyntaxContext,
        },
        import::ImportContext,
    },
};

/// Combines the three result families that can follow a dot.
///
/// Fields and methods retain declaration identity. Postfix rows reuse the same receiver, but
/// replace the whole expression with a synthetic transform such as `value.if` or `value.box`.
pub(super) struct DotCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionContext<'source>,
}

impl<'a, 'db, 'source> DotCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionContext<'source>) -> Self {
        Self { analysis, query }
    }

    /// Collects member completions for a dot site like `user.na$0`.
    pub(super) fn completions(
        &self,
        sites: &[DotCompletionSite],
        syntax_context: Option<&CompletionSyntaxContext<'_>>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let Some((first, copies)) = sites.split_first() else {
            return Ok(Vec::new());
        };
        if copies.is_empty() {
            return self.completions_in_scope(
                *first,
                syntax_context,
                self.query.client_capabilities.auto_import,
            );
        }

        // Repeated copies can have different scopes. Offer the member choices that apply in
        // every scope. Imports and postfix edits have a single source context to resolve against.
        let mut items = self.completions_in_scope(*first, None, false)?;
        for copy in copies {
            if items.is_empty() {
                break;
            }
            let other = self.completions_in_scope(*copy, None, false)?;
            items.retain(|item| other.contains(item));
        }
        Ok(items)
    }

    fn completions_in_scope(
        &self,
        site: DotCompletionSite,
        syntax_context: Option<&CompletionSyntaxContext<'_>>,
        auto_import: bool,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let edit = CompletionEdit {
            replace: site.replace_span(),
        };
        let db = self.analysis.view_db();
        let receiver = site.source().receiver();
        let receiver_ty = BodyView::new(db)
            .expr_ty(receiver.body_ir(), receiver.expr_id())
            .context("read dot receiver type")?;
        let receiver_is_bool =
            receiver_ty.as_ref().and_then(|ty| ty.primitive()) == Some(PrimitiveTy::Bool);
        let postfix = PostfixCompletionResolver::new(self.query).completions(
            &site,
            syntax_context,
            receiver_is_bool,
        );
        let Some(receiver_ty) = receiver_ty else {
            return Ok(postfix);
        };

        let members = MemberView::new(db);
        let scope = site.source().scope();
        let use_crate = receiver.body_ir().crate_ref;
        let ranking = self.query.expected_type;
        let field_type = ranking.and_then(|ranking| ranking.field_result_type());
        let call_completion = CallCompletionKind::MethodCall;
        let call_type = ranking
            .filter(|_| self.query.completes_call(call_completion))
            .and_then(|ranking| ranking.call_result_type());
        let mut completions = Vec::new();

        let syntax = SyntaxRenderer::new(db.crate_edition(self.query.crate_ref)?);
        let field_renderer = FieldCompletionRenderer::new(syntax);
        for (field, type_matches) in members
            .field_candidates_for_ty(receiver.body_ir(), &receiver_ty, field_type)
            .context("collect dot field candidates")?
        {
            if !field.is_suggestible_from(use_crate) {
                continue;
            }
            let Some(mut completion) = field_renderer.completion(field, edit) else {
                continue;
            };
            if completions
                .iter()
                .any(|existing: &CompletionItem| existing.target == completion.target)
            {
                continue;
            }
            if type_matches && let Some(ranking) = ranking {
                ranking.promote_match(&mut completion);
            }
            completions.push(completion);
        }

        let function_renderer = FunctionCompletionRenderer::new(self.query, syntax);
        for method in members
            .method_candidates_for_ty(scope, &receiver_ty, call_type)
            .context("collect dot method candidates")?
        {
            let function = method.function();
            if !function.is_suggestible_from(use_crate) {
                continue;
            }
            let (kind, applicability) = match method.origin() {
                MemberMethodOrigin::Inherent => (
                    CompletionKind::InherentMethod,
                    CompletionApplicability::Known,
                ),
                MemberMethodOrigin::Trait { applicability } => {
                    (CompletionKind::TraitMethod, applicability.into())
                }
            };
            let mut completion = function_renderer.completion(FunctionCompletionRequest {
                function,
                label_override: None,
                kind,
                applicability,
                edit,
                call_completion,
                sort_policy: CompletionSortPolicy::General,
                sort_priority: Some(CompletionSortPriority::Member),
            });
            if method.return_type_matches()
                && let Some(ranking) = ranking
            {
                ranking.promote_call_match(&mut completion);
            }
            completions.push(completion);
        }

        completions.extend(postfix);

        if auto_import
            && let Some(prefix) = self
                .query
                .source_text
                .get(edit.replace.start as usize..self.query.offset as usize)
                .map(|prefix| prefix.strip_prefix("r#").unwrap_or(prefix))
            && let Some(deadline) = self.query.auto_import_deadline(prefix)
            && let Some(import_context) = ImportContext::for_dot_site(db, site.source())?
        {
            let extensions = members.extension_method_candidates(
                scope,
                import_context.module(),
                &receiver_ty,
                call_type,
                prefix,
                deadline,
                |name| {
                    completions.iter().any(|item| {
                        matches!(
                            item.kind,
                            CompletionKind::InherentMethod | CompletionKind::TraitMethod
                        ) && item.label.strip_prefix("r#").unwrap_or(&item.label) == name
                    })
                },
            )?;
            for (method, path) in extensions {
                let mut item = function_renderer.completion(FunctionCompletionRequest {
                    function: method.function(),
                    label_override: None,
                    kind: CompletionKind::TraitMethod,
                    applicability: CompletionApplicability::Known,
                    edit,
                    call_completion,
                    sort_policy: CompletionSortPolicy::General,
                    sort_priority: Some(CompletionSortPriority::AutoImport {
                        path_len: path.component_count(),
                    }),
                });
                if method.return_type_matches()
                    && let Some(ranking) = ranking
                {
                    ranking.promote_call_match(&mut item);
                }
                item.import = Some(CompletionImport {
                    display_path: syntax.path(&path).to_string(),
                    path,
                    anonymous: true,
                });
                completions.push(item);
            }
        }

        Ok(completions)
    }
}
