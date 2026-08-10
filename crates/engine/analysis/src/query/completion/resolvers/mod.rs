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

use crate::{
    Analysis,
    model::{CompletionEdit, CompletionItem},
};
use anyhow::Context as _;

use super::{
    CompletionQuery,
    site::{
        CompletionSite, CompletionSiteDetector, CompletionSiteSyntax, ItemListCompletionKind,
        NameCompletionContext, SpecializedCompletionContext, StandaloneCompletionSiteSyntax,
        SyntaxCompletionContext,
    },
    syntax::CompletionSyntaxContextCache,
};

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

/// Coordinates completion-site detection with semantic candidate rendering.
///
/// The resolver first gives narrow request-only grammars such as attributes, strings, lifetimes,
/// and restricted visibility a chance to claim the cursor. For ordinary Rust names it passes
/// decisive syntax hints to `CompletionSiteDetector`, which joins the incomplete request shape to
/// an indexed body, signature, import, module, or trait-impl site. Only after that routing step does
/// the matching resolver perform semantic lookup and render rows.
pub(crate) struct CompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> CompletionResolver<'a, 'db, 'source> {
    pub(crate) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Collects completions for one source offset, e.g. `user.$0`,
    /// `let value = crate::$0`, `let value = inp$0`, `User { na$0 }`, or `use st$0`.
    pub(crate) fn completions_at(&self) -> anyhow::Result<Vec<CompletionItem>> {
        let mut syntax_context =
            CompletionSyntaxContextCache::new(self.query.source_text, self.query.offset);

        // Item lists and specialized declaration syntax are owned by the request-local classifier.
        // An incomplete `extern c$0`, for example, can resemble a lowered extern-crate name even
        // though the valid completion at this point is the `crate` keyword.
        let syntax_domain = syntax_context
            .get()
            .and_then(|syntax| syntax.completion_context());
        let is_plain_item_list = matches!(
            syntax_domain.as_ref(),
            Some(SyntaxCompletionContext::ItemList(context))
                if context.kind() != ItemListCompletionKind::TraitImpl
        );
        if is_plain_item_list {
            return KeywordCompletionResolver::new(self.query.client_capabilities)
                .completions(syntax_context.get());
        }
        if let Some(SyntaxCompletionContext::Specialized(context)) = syntax_domain.as_ref() {
            let Some(syntax) = syntax_context.get() else {
                return Ok(Vec::new());
            };
            return match context {
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
            };
        }

        // Keyword fragments can be useful even when the cursor does not lower
        // into a semantic completion site. For example, `f$0` at item level is
        // just incomplete text, not a Body IR or DefMap path.
        let syntax_hint = syntax_context.get().map(|syntax| {
            let context = syntax.completion_context();
            let empty_path = match context.as_ref() {
                Some(SyntaxCompletionContext::EmptyPath(context)) => Some(*context),
                Some(
                    SyntaxCompletionContext::Type(_)
                    | SyntaxCompletionContext::Pattern(_)
                    | SyntaxCompletionContext::ItemList(_)
                    | SyntaxCompletionContext::BodyMacro(_)
                    | SyntaxCompletionContext::ModuleMacro(_)
                    | SyntaxCompletionContext::ModuleDeclaration(_)
                    | SyntaxCompletionContext::Statement
                    | SyntaxCompletionContext::Expression
                    | SyntaxCompletionContext::Specialized(_),
                )
                | None => None,
            };
            let standalone = match context {
                Some(SyntaxCompletionContext::ItemList(context))
                    if context.kind() == ItemListCompletionKind::TraitImpl =>
                {
                    syntax
                        .trait_impl_completion_syntax()
                        .map(StandaloneCompletionSiteSyntax::TraitImpl)
                }
                Some(SyntaxCompletionContext::BodyMacro(context)) => {
                    Some(StandaloneCompletionSiteSyntax::BodyMacro {
                        qualifier: context.qualifier().cloned(),
                    })
                }
                Some(SyntaxCompletionContext::ModuleMacro(context)) => {
                    Some(StandaloneCompletionSiteSyntax::ModuleMacro {
                        qualifier: context.qualifier().cloned(),
                    })
                }
                Some(SyntaxCompletionContext::ModuleDeclaration(context)) => {
                    Some(StandaloneCompletionSiteSyntax::ModuleDeclaration {
                        has_path_attribute: context.has_path_attribute(),
                    })
                }
                Some(
                    SyntaxCompletionContext::EmptyPath(_)
                    | SyntaxCompletionContext::Type(_)
                    | SyntaxCompletionContext::Pattern(_)
                    | SyntaxCompletionContext::ItemList(_)
                    | SyntaxCompletionContext::Statement
                    | SyntaxCompletionContext::Expression
                    | SyntaxCompletionContext::Specialized(_),
                )
                | None => None,
            };
            let prefix = syntax.prefix();
            CompletionSiteSyntax::new(
                syntax.inside_use_item(),
                syntax.after_dot(),
                syntax.after_colon_colon(),
                syntax.empty_qualified_path(),
                empty_path,
                syntax.empty_record_owner(),
                syntax.body_owner_start(),
                standalone,
                prefix.span(),
                prefix.text().to_string(),
            )
        });
        let Some(site) = CompletionSiteDetector::new(self.analysis.view_db())
            .site_at(
                self.query.crate_ref,
                self.query.file_id,
                self.query.offset,
                syntax_hint,
            )
            .context("detect completion site")?
        else {
            return KeywordCompletionResolver::new(self.query.client_capabilities)
                .completions(syntax_context.get());
        };

        match site {
            CompletionSite::AssociatedTypeBinding(site) => {
                AssociatedTypeBindingCompletionResolver::new(self.analysis, self.query)
                    .completions(site)
            }
            CompletionSite::Dot(site) => DotCompletionResolver::new(self.analysis, self.query)
                .completions(site, syntax_context.get()),
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
                if let Some(SyntaxCompletionContext::ModuleMacro(context)) = syntax_domain.as_ref()
                    && let Some(item_list) = context.incomplete_item_list()
                {
                    completions.extend(
                        KeywordCompletionResolver::new(self.query.client_capabilities)
                            .item_list_completions(item_list, syntax_context.get())
                            .context("collect item keywords beside incomplete module macro")?,
                    );
                    completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
                }
                Ok(completions)
            }
            CompletionSite::Path(site) => {
                PathCompletionResolver::new(self.analysis, self.query).completions(site)
            }
            CompletionSite::TraitImpl(site) => {
                let mut completions = TraitImplCompletionResolver::new(self.analysis, self.query)
                    .completions(site)
                    .context("collect trait impl completions")?;
                completions.extend(
                    KeywordCompletionResolver::new(self.query.client_capabilities)
                        .completions(syntax_context.get())
                        .context("collect trait impl keyword completions")?,
                );
                completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
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
                let mut completions = UnqualifiedCompletionResolver::new(self.analysis, self.query)
                    .completions(site)
                    .context("collect unqualified completions")?;
                if context == NameCompletionContext::Type
                    && let Some(binding) =
                        rg_ir_view::source::SourceCompletionView::new(self.analysis.view_db())
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
                            .pattern_overlay_completions(syntax_context.get())
                            .context("collect pattern keyword completions")?,
                        NameCompletionContext::Type => keywords
                            .type_overlay_completions(syntax_context.get())
                            .context("collect type keyword completions")?,
                        NameCompletionContext::Value => keywords
                            .overlay_completions(syntax_context.get())
                            .context("collect expression keyword completions")?,
                        NameCompletionContext::Const => Vec::new(),
                        NameCompletionContext::Import => Vec::new(),
                    });
                    completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
                }
                let keywords = KeywordCompletionResolver::new(self.query.client_capabilities);
                for root in keywords
                    .path_root_overlay_completions(&path_root_prefix, path_root_edit)
                    .context("collect path root keyword completions")?
                {
                    if !completions
                        .iter()
                        .any(|existing| existing.label == root.label)
                    {
                        completions.push(root);
                    }
                }
                completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
                Ok(completions)
            }
            CompletionSite::RecordField(site) => {
                RecordFieldCompletionResolver::new(self.analysis.view_db(), self.query.crate_ref)
                    .completions(site)
            }
        }
    }
}
