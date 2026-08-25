//! Exact-name imports for unresolved ordinary type and value names.
//!
//! ```text
//! fn load(_: User) {}
//! // action: Import `crate::models::User`
//! ```
//!
//! The provider first proves that the whole token is an ordinary unresolved name, then reuses
//! completion's scope classification, bounded import discovery, and conservative source-edit
//! planner. Passive lightbulb probes do not perform the graph walk.

use anyhow::Context as _;
use rg_ir_view::{
    SymbolKind, display::syntax::SyntaxRenderer, lookup::importable::ImportableNameSearch,
};
use rg_parse::Span;
use rg_syntax::{AstNode as _, ast};

use crate::{
    Analysis, CodeAction, CodeActionKind, CodeActionQuery, CodeActionTrigger,
    query::{
        completion::CompletionSiteDetector,
        import::{ImportContext, ImportEditPlan, ImportEditPlanner},
    },
};

use super::syntax::CodeActionSyntax;

/// Turns one unresolved name into one action for each concrete `use` path that can provide it.
///
/// For `User`, search may find both `crate::api::User` and `other_crate::User`. Each path is shown
/// separately, and only paths the import planner can add without occupying an existing name are
/// returned.
pub(super) struct ImportItemCodeActionProvider<'analysis, 'db, 'source> {
    analysis: &'analysis Analysis<'db>,
    query: CodeActionQuery<'source>,
}

impl<'analysis, 'db, 'source> ImportItemCodeActionProvider<'analysis, 'db, 'source> {
    pub(super) fn new(analysis: &'analysis Analysis<'db>, query: CodeActionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Offer exact imports only after an explicit request for one complete unresolved name.
    ///
    /// Several declarations may have the same visible spelling, so each safe path becomes a
    /// separate action. A sole result is preferred; analysis does not guess between alternatives.
    pub(super) fn code_actions(
        &self,
        syntax: &CodeActionSyntax<'_>,
    ) -> anyhow::Result<Vec<CodeAction>> {
        if self.query.trigger != CodeActionTrigger::Invoked {
            return Ok(Vec::new());
        }

        // 1. Select one complete source token. A standalone name gets its type/value namespace
        // from completion. The first segment of `HashMap::new`, however, is not a standalone
        // completion site: Rust resolves every non-final path segment in the type namespace. Ask
        // the import context to recognize that shape and find the module that would receive its
        // `use` item.
        let Some(path_name) = syntax.path_name_at_request() else {
            return Ok(Vec::new());
        };
        let name_ref = path_name.name();
        if name_ref
            .syntax()
            .ancestors()
            .any(|node| ast::MacroCall::can_cast(node.kind()) || ast::Use::can_cast(node.kind()))
        {
            return Ok(Vec::new());
        }
        let name_span = Span::from_text_range(name_ref.syntax().text_range());
        let site = CompletionSiteDetector::new(self.analysis)
            .unqualified_name_for_source(
                self.query.crate_ref,
                self.query.file_id,
                syntax.source(),
                name_span.text.end,
            )
            .context("classify unresolved import action name")?;

        let (import_context, lookup_name, definition_offset) = if let Some(site) = site {
            // Completion's speculative parser strips `r#` from its identifier prefix. Accept that
            // one source spelling, but require the classified prefix to describe this entire token.
            let replace = site.replace_span();
            let Ok(name_start) = usize::try_from(name_span.text.start) else {
                return Ok(Vec::new());
            };
            let Ok(replace_start) = usize::try_from(replace.text.start) else {
                return Ok(Vec::new());
            };
            let Some(raw_prefix) = syntax.source().get(name_start..replace_start) else {
                return Ok(Vec::new());
            };
            if replace.text.end != name_span.text.end
                || !(replace.text.start == name_span.text.start || raw_prefix == "r#")
                || site.member_prefix().is_empty()
            {
                return Ok(Vec::new());
            }

            let Some(import_context) =
                ImportContext::for_unqualified_site(self.analysis.view_db(), site.source())
                    .context("read exact import action context")?
            else {
                return Ok(Vec::new());
            };
            (
                import_context,
                site.member_prefix().to_string(),
                replace.text.start,
            )
        } else {
            // Completion treats names after `::` as qualified sites and deliberately does not
            // reinterpret the first name as an ordinary unqualified site. Ask the import domain
            // whether the selected token is such a root and which current module would receive it.
            let Some(import_context) = ImportContext::for_qualified_root(
                self.analysis.view_db(),
                self.query.crate_ref,
                self.query.file_id,
                path_name.segment(),
            )
            .context("read qualified-root import action context")?
            else {
                return Ok(Vec::new());
            };
            let written_name = name_ref.text().to_string();
            let lookup_name = written_name
                .strip_prefix("r#")
                .unwrap_or(&written_name)
                .to_string();
            if lookup_name.is_empty() {
                return Ok(Vec::new());
            }
            (import_context, lookup_name, name_span.text.start)
        };

        // 2. A local, generic, existing import, or module item already occupying the name must win.
        // Import discovery is only a repair for names with no definition at this exact site.
        if !self
            .analysis
            .goto_definition(self.query.crate_ref, self.query.file_id, definition_offset)
            .context("check whether import action name already resolves")?
            .is_empty()
        {
            return Ok(Vec::new());
        }
        let edition = self
            .analysis
            .view_db()
            .crate_edition(self.query.crate_ref)
            .context("read exact import action edition")?;
        let syntax_renderer = SyntaxRenderer::new(edition);
        let planner =
            ImportEditPlanner::for_source(syntax.source(), syntax.file(), name_span.text.start);

        // 3. Find declarations with this exact name and ask the shared planner whether each path
        // can become a `use` in the module containing the unresolved reference.
        let mut actions = Vec::new();
        for candidate in ImportableNameSearch::new(self.analysis.view_db())
            .search_exact(import_context.module(), &lookup_name)
            .context("search exact import action candidates")?
        {
            if candidate.name().namespace() != import_context.namespace()
                || candidate.name().kind() == SymbolKind::EnumVariant
            {
                continue;
            }

            let rendered_path = syntax_renderer.path(candidate.path()).to_string();
            let ImportEditPlan::Edit(edit) = planner.plan(candidate.path(), &rendered_path) else {
                continue;
            };
            actions.push(CodeAction {
                title: format!("Import `{rendered_path}`"),
                kind: CodeActionKind::QuickFix,
                is_preferred: false,
                edits: vec![edit],
            });
        }

        if actions.len() == 1 {
            actions[0].is_preferred = true;
        }
        Ok(actions)
    }
}
