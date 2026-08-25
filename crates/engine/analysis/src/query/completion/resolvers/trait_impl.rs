//! Missing trait-member completion for a resolved trait implementation.
//!
//! ```text
//! impl Service<u8> for Worker {
//!     fn req$0
//! }
//!
//! // Accepting `required` inserts:
//! fn required(&self, value: u8) -> Self::Output {
//!     todo!()
//! }
//! ```
//!
//! The view layer supplies substituted, syntax-shaped scaffolds. This module owns editor policy:
//! a written `fn`, `type`, or `const` selects the corresponding member family and is replaced
//! together with the partial name. The lookup key repeats that written introducer so the editor
//! can match `fn re` against `fn required` even though the displayed label is only `required`.
//! Required members sort before defaulted ones; snippet placeholders are added when supported;
//! and every row retains the trait declaration as its navigation/documentation target.

use anyhow::Context as _;
use rg_ir_view::trait_impl::{MissingTraitMemberRef, MissingTraitMemberScaffold};

use crate::{
    Analysis,
    model::{
        CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem,
        CompletionKind, CompletionTarget,
    },
    query::completion::site::{TraitImplCompletionSite, TraitImplMemberKind},
    query::trait_member::{RenderedTraitMember, TraitImplMemberQuery},
};

use super::super::{
    CompletionQuery,
    render::{CompletionSortPolicy, escape_lsp_snippet_text},
};

/// Turns missing trait declarations into editor-ready implementation scaffolds.
///
/// The view layer has already substituted trait generics. This resolver chooses the member family
/// implied by a written `fn`, `type`, or `const`, then applies replacement and snippet policy.
pub(super) struct TraitImplCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> TraitImplCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Render each missing declaration whose family matches the written member prefix.
    pub(super) fn completions(
        &self,
        site: TraitImplCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let Some(source_text) = self.query.source_text else {
            return Ok(Vec::new());
        };
        let members = TraitImplMemberQuery::new(
            self.analysis,
            self.query.crate_ref,
            self.query.file_id,
            source_text,
        )
        .missing_members_at(site.owner_start())
        .context("collect missing trait member completions")?;
        let edit = CompletionEdit {
            replace: site.replace_span(),
        };
        let mut completions = Vec::new();

        for member in members {
            let (member_kind, kind, target) = match member.member() {
                MissingTraitMemberRef::Function(function) => (
                    TraitImplMemberKind::Function,
                    CompletionKind::Function,
                    CompletionTarget::Function(function),
                ),
                MissingTraitMemberRef::TypeAlias(alias) => (
                    TraitImplMemberKind::TypeAlias,
                    CompletionKind::TypeAlias,
                    CompletionTarget::Declaration(alias.into()),
                ),
                MissingTraitMemberRef::Const(konst) => (
                    TraitImplMemberKind::Const,
                    CompletionKind::Const,
                    CompletionTarget::Declaration(konst.into()),
                ),
            };
            if site
                .member_kind()
                .is_some_and(|written_kind| written_kind != member_kind)
            {
                continue;
            }
            let rendered = RenderedTraitMember::new(member.scaffold());
            let snippet = match member.scaffold() {
                MissingTraitMemberScaffold::Function { signature } => format!(
                    "{} {{\n    ${{1:todo!()}}\n}}",
                    escape_lsp_snippet_text(signature)
                ),
                MissingTraitMemberScaffold::TypeAlias {
                    signature_prefix,
                    suggested_value,
                } => format!(
                    "{} = ${{1:{}}};",
                    escape_lsp_snippet_text(signature_prefix),
                    escape_lsp_snippet_text(suggested_value)
                ),
                MissingTraitMemberScaffold::Const { signature } => {
                    format!("{} = ${{1:todo!()}};", escape_lsp_snippet_text(signature))
                }
            };
            let requirement = if member.is_required() {
                "required"
            } else {
                "default"
            };
            let base_sort = CompletionSortPolicy::General.sort_text(
                None,
                member.label(),
                kind,
                CompletionApplicability::Known,
                target,
            );
            completions.push(CompletionItem {
                label: member.label().to_string(),
                filter_text: site.filter_text(member.label()),
                kind,
                target,
                applicability: CompletionApplicability::Known,
                detail: Some(format!(
                    "{requirement} trait member: {}",
                    rendered.signature
                )),
                documentation: member.documentation().map(ToString::to_string),
                sort_text: format!(
                    "{}|{base_sort}",
                    if member.is_required() {
                        "00-required"
                    } else {
                        "01-default"
                    }
                ),
                insert_text: if self.query.client_capabilities.snippet_support {
                    CompletionInsertText::Snippet(snippet)
                } else {
                    CompletionInsertText::Text(rendered.plain)
                },
                edit: Some(edit),
                additional_edits: Vec::new(),
            });
        }

        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }
}
