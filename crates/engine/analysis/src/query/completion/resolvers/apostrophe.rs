//! Lifetime and loop-label completion over their distinct scope sources.
//!
//! ```text
//! fn borrow<'a>(value: &'a str) -> &'$0 str
//!
//! 'scan: loop {
//!     break 'sc$0;
//! }
//! ```
//!
//! Both positions begin with an apostrophe, but they do not share a namespace. Lifetime rows come
//! from declaration generics, higher-ranked binders, `'static`, and `'_'`; label rows come only
//! from enclosing loop-like body expressions. The syntax classifier makes that distinction before
//! this resolver asks either scope.

use anyhow::Context as _;
use rg_ir_view::{
    lookup::name::NameLookupView,
    source::{IndexedUnqualifiedNameContext, IndexedUnqualifiedNameScope, SourceCompletionView},
    ty::locals::BodyView,
};

use crate::{
    Analysis,
    model::{
        CompletionEdit, CompletionInsertText, CompletionItem, CompletionKind, CompletionTarget,
        SyntheticCompletionTarget,
    },
    query::completion::site::{
        CompletionSourceAttachment, LabelCompletionContext, LifetimeCompletionContext,
    },
};

use super::super::{
    CompletionQuery,
    render::{SyntheticCompletionCandidate, SyntheticCompletionRenderer},
    syntax::CompletionSyntaxContext,
};

/// Chooses the semantic scope that matches an already-classified apostrophe site.
///
/// Lifetime rows combine generic and higher-ranked binders. Label rows instead walk enclosing
/// loop expressions, so the two namespaces never share candidates merely because both use `'`.
pub(super) struct ApostropheCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> ApostropheCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Complete visible lifetime parameters together with `'static`, `'_`, and local binders.
    pub(super) fn lifetime_completions(
        &self,
        context: &LifetimeCompletionContext,
        syntax: &CompletionSyntaxContext<'_>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        if context.is_declaration() {
            return Ok(Vec::new());
        }
        let prefix = syntax.prefix();
        let edit = CompletionEdit {
            replace: prefix.span(),
        };
        let mut candidates = vec![
            Self::apostrophe_candidate(
                "'static",
                CompletionKind::Lifetime,
                SyntheticCompletionTarget::Lifetime,
            ),
            Self::apostrophe_candidate(
                "'_",
                CompletionKind::Lifetime,
                SyntheticCompletionTarget::Lifetime,
            ),
        ];
        for lifetime in context.binder_lifetimes() {
            if !candidates.iter().any(|candidate| {
                // This equality is only for request-local duplicate suppression; renderer also
                // keeps semantic targets distinct where labels differ.
                candidate.label() == lifetime
            }) {
                candidates.push(Self::apostrophe_candidate(
                    lifetime,
                    CompletionKind::Lifetime,
                    SyntheticCompletionTarget::Lifetime,
                ));
            }
        }

        if let Some(owner) = self
            .generic_owner_at(prefix.span(), prefix.text())
            .context("find lifetime completion owner")?
        {
            for lifetime in NameLookupView::new(self.analysis.view_db())
                .lifetime_scope_names(owner)
                .context("collect visible lifetime completions")?
            {
                if candidates
                    .iter()
                    .any(|candidate| candidate.label() == lifetime.label())
                {
                    continue;
                }
                candidates.push(
                    Self::apostrophe_candidate(
                        lifetime.label(),
                        CompletionKind::Lifetime,
                        SyntheticCompletionTarget::Lifetime,
                    )
                    .with_target(CompletionTarget::GenericParam(lifetime.target())),
                );
            }
        }

        Ok(SyntheticCompletionRenderer::new(prefix.text(), edit).completions(candidates))
    }

    /// Complete labels from enclosing loop-like expressions only.
    pub(super) fn label_completions(
        &self,
        context: LabelCompletionContext,
        syntax: &CompletionSyntaxContext<'_>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        if context.is_declaration() {
            return Ok(Vec::new());
        }
        let prefix = syntax.prefix();
        let edit = CompletionEdit {
            replace: prefix.span(),
        };
        let candidates = SourceCompletionView::new(self.analysis.view_db())
            .enclosing_labels_at(self.query.crate_ref, self.query.file_id, self.query.offset)
            .context("collect enclosing label completions")?
            .into_iter()
            .map(|label| {
                Self::apostrophe_candidate(
                    &label,
                    CompletionKind::Label,
                    SyntheticCompletionTarget::Label,
                )
            });
        Ok(SyntheticCompletionRenderer::new(prefix.text(), edit).completions(candidates))
    }

    fn generic_owner_at(
        &self,
        prefix_span: rg_parse::Span,
        prefix: &str,
    ) -> anyhow::Result<Option<rg_ir_model::GenericDefRef>> {
        let source = SourceCompletionView::new(self.analysis.view_db());
        if let Some(site) = source
            .body_syntax_name_site_at(
                self.query.crate_ref,
                self.query.file_id,
                self.query.offset,
                IndexedUnqualifiedNameContext::Const,
                prefix_span,
                prefix.to_string(),
            )
            .context("find body lifetime completion site")?
            && let IndexedUnqualifiedNameScope::Body {
                scope,
                generic_owner,
                ..
            } = site.scope()
        {
            if let Some(owner) = generic_owner {
                return Ok(Some(*owner));
            }
            return BodyView::new(self.analysis.view_db())
                .generic_owner(scope.body_ir())
                .context("read body lifetime completion owner");
        }

        Ok(
            CompletionSourceAttachment::new(
                self.analysis,
                self.query.crate_ref,
                self.query.file_id,
            )
            .signature_name_site_at(
                self.query.offset,
                IndexedUnqualifiedNameContext::Const,
                prefix_span,
                prefix.to_string(),
            )
            .context("find signature lifetime completion site")?
            .and_then(|site| match site.scope() {
                IndexedUnqualifiedNameScope::Signature { scope, .. } => Some(scope.generic_owner()),
                IndexedUnqualifiedNameScope::Body { .. }
                | IndexedUnqualifiedNameScope::Module { .. }
                | IndexedUnqualifiedNameScope::Import { .. } => None,
            }),
        )
    }

    fn apostrophe_candidate(
        label: &str,
        kind: CompletionKind,
        target: SyntheticCompletionTarget,
    ) -> SyntheticCompletionCandidate {
        let name = label.strip_prefix('\'').unwrap_or(label);
        SyntheticCompletionCandidate::new(label, kind, target)
            .with_match_text(name)
            // The replacement starts after the already-written apostrophe.
            .with_insert_text(CompletionInsertText::Text(name.to_string()))
            .with_detail(format!("{kind} {label}"))
    }
}
