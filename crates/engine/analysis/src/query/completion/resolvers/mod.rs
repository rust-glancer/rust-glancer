//! Completion-family routing and result-producing resolvers.
//!
//! The coordinator chooses one primary completion family from request syntax and indexed source
//! sites. Each child resolver owns the final filtering and assembly for that family, including any
//! small overlays that are meaningful at the same cursor position.

mod apostrophe;
mod associated_type_binding;
mod attribute;
mod const_expression;
mod dot;
mod keyword;
mod module_declaration;
mod module_macro;
mod path;
mod postfix;
mod record;
mod specialized;
mod trait_impl;
mod unqualified;
mod visibility;

use anyhow::Context as _;
use rg_ir_view::source::SourceCompletionView;

use self::{
    apostrophe::ApostropheCompletionResolver,
    associated_type_binding::AssociatedTypeBindingCompletionResolver,
    attribute::AttributeCompletionResolver, const_expression::ConstExpressionCompletionResolver,
    dot::DotCompletionResolver, keyword::KeywordCompletionResolver,
    module_declaration::ModuleDeclarationCompletionResolver,
    module_macro::ModuleMacroCompletionResolver, path::PathCompletionResolver,
    record::RecordFieldCompletionResolver, specialized::SpecializedCompletionResolver,
    trait_impl::TraitImplCompletionResolver, unqualified::UnqualifiedCompletionResolver,
    visibility::VisibilityCompletionResolver,
};
use super::{
    CompletionContext, CompletionQuery,
    expected_type::ExpectedTypeRanking,
    site::{
        CompletionSite, CompletionSiteDetector, ItemListCompletionKind, NameCompletionContext,
        SpecializedCompletionContext, SyntaxCompletionContext,
    },
    syntax::CompletionSyntaxContext,
};
use crate::{
    Analysis,
    model::{CompletionEdit, CompletionResult},
};

/// Coordinates completion-site detection with semantic candidate rendering.
///
/// The resolver first gives narrow request-only grammars such as attributes, strings, lifetimes,
/// and restricted visibility a chance to claim the cursor. For ordinary Rust names it passes
/// decisive syntax hints to `CompletionSiteDetector`, which joins the incomplete request shape to
/// an indexed body, signature, import, module, or trait-impl site. Only after that routing step does
/// the matching resolver perform semantic lookup and render rows.
pub(crate) struct CompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionContext<'source>,
    shared_syntax: Option<&'source CompletionSyntaxContext<'source>>,
}

impl<'a, 'db, 'source> CompletionResolver<'a, 'db, 'source> {
    pub(crate) fn new(
        analysis: &'a Analysis<'db>,
        query: CompletionQuery<'source>,
        source_text: &'source str,
    ) -> Self {
        Self {
            analysis,
            shared_syntax: query.completion_source().map(|source| &source.syntax),
            query: CompletionContext {
                crate_ref: query.crate_ref,
                file_id: query.file_id,
                offset: query.offset,
                source_text,
                client_capabilities: query.client_capabilities,
                auto_import_budget: query.auto_import_budget,
                expected_type: None,
                call_suffix: Default::default(),
            },
        }
    }

    /// Collects completions for one source offset, e.g. `user.$0`,
    /// `let value = crate::$0`, `let value = inp$0`, `User { na$0 }`, or `use st$0`.
    pub(crate) fn completions_at(&self) -> anyhow::Result<CompletionResult> {
        let complete = |items| CompletionResult {
            items,
            is_incomplete: false,
        };
        let local_syntax = self
            .shared_syntax
            .is_none()
            .then(|| CompletionSyntaxContext::at(self.query.source_text, self.query.offset))
            .flatten();
        let syntax_context = self.shared_syntax.or(local_syntax.as_ref());

        // Item lists and specialized declaration syntax are owned by the request-local classifier.
        // An incomplete `extern c$0`, for example, can resemble a lowered extern-crate name even
        // though the valid completion at this point is the `crate` keyword.
        let syntax_domain = syntax_context.and_then(|syntax| syntax.completion_context());
        let is_plain_item_list = matches!(
            syntax_domain.as_ref(),
            Some(SyntaxCompletionContext::ItemList(context))
                if context.kind() != ItemListCompletionKind::TraitImpl
        );
        if is_plain_item_list {
            return KeywordCompletionResolver::new(self.query.client_capabilities)
                .completions(syntax_context)
                .map(complete);
        }
        if let Some(SyntaxCompletionContext::Specialized(context)) = syntax_domain {
            let Some(syntax) = syntax_context else {
                return Ok(CompletionResult::default());
            };
            let items = match context {
                SpecializedCompletionContext::Attribute(context) => {
                    AttributeCompletionResolver::new(self.analysis, self.query)
                        .completions(context, syntax)
                }
                SpecializedCompletionContext::ConstExpression(context) => {
                    ConstExpressionCompletionResolver::new(self.analysis, self.query)
                        .completions(context, syntax)
                }
                SpecializedCompletionContext::ExternCrateName => {
                    VisibilityCompletionResolver::new(self.analysis, self.query)
                        .extern_crate_completions(syntax)
                }
                SpecializedCompletionContext::Label(context) => {
                    ApostropheCompletionResolver::new(self.analysis, self.query)
                        .label_completions(*context, syntax)
                }
                SpecializedCompletionContext::Lifetime(context) => {
                    ApostropheCompletionResolver::new(self.analysis, self.query)
                        .lifetime_completions(context, syntax)
                }
                SpecializedCompletionContext::RestrictedVisibility(context) => {
                    VisibilityCompletionResolver::new(self.analysis, self.query)
                        .restricted_visibility_completions(context, syntax)
                }
                SpecializedCompletionContext::MacroFragment => Ok(
                    SpecializedCompletionResolver::new(self.analysis, self.query)
                        .macro_fragment_completions(syntax),
                ),
                SpecializedCompletionContext::String(context) => {
                    SpecializedCompletionResolver::new(self.analysis, self.query)
                        .string_completions(context, syntax)
                }
            }?;
            return Ok(complete(items));
        }

        // Keyword fragments can be useful even when the cursor does not lower
        // into a semantic completion site. For example, `f$0` at item level is
        // just incomplete text, not a Body IR or DefMap path.
        let syntax_hint = syntax_context.map(CompletionSyntaxContext::site_syntax);
        let Some(site) = CompletionSiteDetector::new(self.analysis)
            .site_at(
                self.query.crate_ref,
                self.query.file_id,
                self.query.offset,
                syntax_hint,
            )
            .context("detect completion site")?
        else {
            return KeywordCompletionResolver::new(self.query.client_capabilities)
                .completions(syntax_context)
                .map(complete);
        };

        // Auto-import depends on the prefix. Even a list with only in-scope names
        // at `H` needs a new search once the user reaches `Ha`.
        let is_incomplete = self.query.client_capabilities.auto_import
            && match &site {
                CompletionSite::Dot(sites) => sites.len() == 1,
                CompletionSite::Unqualified(site) => site.context().supports_auto_import(),
                _ => false,
            };
        let ranking = ExpectedTypeRanking::at(self.analysis, self.query, syntax_context, &site)?;
        let query = CompletionContext {
            expected_type: ranking.as_ref(),
            call_suffix: syntax_context
                .map(CompletionSyntaxContext::call_suffix)
                .unwrap_or_default(),
            ..self.query
        };
        let mut items = match site {
            CompletionSite::AssociatedTypeBinding(site) => {
                AssociatedTypeBindingCompletionResolver::new(self.analysis, self.query)
                    .completions(site)
            }
            CompletionSite::Dot(sites) => {
                DotCompletionResolver::new(self.analysis, query).completions(&sites, syntax_context)
            }
            CompletionSite::ModuleDeclaration(site) => ModuleDeclarationCompletionResolver::new(
                self.analysis,
                self.query.crate_ref,
                self.query.file_id,
            )
            .completions(site),
            CompletionSite::ModuleMacro(site) => {
                let mut completions = ModuleMacroCompletionResolver::new(self.analysis, self.query)
                    .completions(site)
                    .context("collect module macro completions")?;
                if let Some(SyntaxCompletionContext::ModuleMacro(context)) = syntax_domain
                    && let Some(item_list) = context.incomplete_item_list()
                {
                    completions.extend(
                        KeywordCompletionResolver::new(self.query.client_capabilities)
                            .item_list_completions(item_list, syntax_context)
                            .context("collect item keywords beside incomplete module macro")?,
                    );
                }
                Ok(completions)
            }
            CompletionSite::Path(site) => {
                PathCompletionResolver::new(self.analysis, query).completions(site)
            }
            CompletionSite::TraitImpl(site) => {
                let mut completions = TraitImplCompletionResolver::new(self.analysis, self.query)
                    .completions(site)
                    .context("collect trait impl completions")?;
                completions.extend(
                    KeywordCompletionResolver::new(self.query.client_capabilities)
                        .completions(syntax_context)
                        .context("collect trait impl keyword completions")?,
                );
                Ok(completions)
            }
            CompletionSite::Unqualified(site) => {
                // Plain body names come from lexical scope, but value positions
                // also accept expression keywords. Keep those as low-priority
                // overlay rows so semantic names remain the primary signal.
                let context = site.context();
                let include_keyword_overlay = site.includes_keyword_overlay();
                let path_root_prefix = site.member_prefix().to_string();
                let path_root_edit = CompletionEdit {
                    replace: site.replace_span(),
                };
                let mut completions = UnqualifiedCompletionResolver::new(self.analysis, query)
                    .completions(site)
                    .context("collect unqualified completions")?;
                if context == NameCompletionContext::Type
                    && let Some(binding) = SourceCompletionView::new(self.analysis.view_db())
                        .implicit_associated_type_binding_site_at(
                            self.query.crate_ref,
                            self.query.file_id,
                            self.query.offset,
                        )
                        .context("detect implicit associated type binding")?
                {
                    completions.extend(
                        AssociatedTypeBindingCompletionResolver::new(self.analysis, self.query)
                            .completions(binding)
                            .context("collect implicit associated type binding completions")?,
                    );
                }
                if include_keyword_overlay {
                    let keywords = KeywordCompletionResolver::new(self.query.client_capabilities);
                    completions.extend(match context {
                        NameCompletionContext::Pattern(_) => keywords
                            .pattern_overlay_completions(syntax_context)
                            .context("collect pattern keyword completions")?,
                        NameCompletionContext::Type => keywords
                            .type_overlay_completions(syntax_context)
                            .context("collect type keyword completions")?,
                        NameCompletionContext::Value => keywords
                            .overlay_completions(syntax_context)
                            .context("collect expression keyword completions")?,
                        NameCompletionContext::Const => Vec::new(),
                        NameCompletionContext::Import => Vec::new(),
                    });
                }
                let keywords = KeywordCompletionResolver::new(self.query.client_capabilities);
                for root in keywords
                    .path_root_overlay_completions(&path_root_prefix, path_root_edit)
                    .context("collect path root keyword completions")?
                {
                    rg_std::check_cancel!(self.analysis, "completion candidate");
                    if !completions
                        .iter()
                        .any(|existing| existing.label == root.label)
                    {
                        completions.push(root);
                    }
                }
                Ok(completions)
            }
            CompletionSite::RecordField(site) => {
                RecordFieldCompletionResolver::new(self.analysis.view_db(), self.query.crate_ref)
                    .completions(site)
            }
        }?;
        if let Some(ranking) = ranking {
            ranking.rank(self.analysis, &mut items)?;
        }
        Ok(CompletionResult {
            items,
            is_incomplete,
        })
    }
}
