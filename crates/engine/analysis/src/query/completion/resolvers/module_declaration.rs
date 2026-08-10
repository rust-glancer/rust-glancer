//! Filesystem-backed completion for conventional out-of-line module declarations.
//!
//! ```text
//! src/
//! ├── lib.rs       // contains `mod par$0`
//! └── parser.rs    // contributes the `parser` completion
//! ```
//!
//! Candidate discovery is request-scoped and follows the same `name.rs` / `name/mod.rs` rules as
//! module loading. Already declared children are removed. A declaration carrying `#[path = ...]`
//! is intentionally excluded because its filename no longer follows those conventions.

use anyhow::Context as _;
use rg_ir_view::display::syntax::SyntaxRenderer;

use crate::{
    Analysis,
    model::{
        CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem,
        CompletionKind, CompletionTarget, SyntheticCompletionTarget,
    },
    query::completion::site::ModuleDeclarationCompletionSite,
};

use super::super::render::{CompletionSortPolicy, definition_detail};

/// Discovers conventional sibling module files and renders their stem as `mod name` candidates.
pub(super) struct ModuleDeclarationCompletionResolver<'a, 'db> {
    analysis: &'a Analysis<'db>,
    crate_ref: rg_ir_model::CrateRef,
    file_id: rg_parse::FileId,
}

impl<'a, 'db> ModuleDeclarationCompletionResolver<'a, 'db> {
    pub(super) fn new(
        analysis: &'a Analysis<'db>,
        crate_ref: rg_ir_model::CrateRef,
        file_id: rg_parse::FileId,
    ) -> Self {
        Self {
            analysis,
            crate_ref,
            file_id,
        }
    }

    /// Complete undeclared `name.rs` and `name/mod.rs` siblings for `mod na$0`.
    pub(super) fn completions(
        &self,
        site: ModuleDeclarationCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        // A path attribute chooses an arbitrary filename, so conventional sibling discovery would
        // be misleading until attribute-aware completion has a dedicated design.
        if site.has_path_attribute() {
            return Ok(Vec::new());
        }

        let syntax = SyntaxRenderer::new(
            self.analysis
                .view_db()
                .crate_edition(self.crate_ref)
                .context("read module declaration edition")?,
        );
        let target = CompletionTarget::Synthetic(SyntheticCompletionTarget::ModuleDeclaration);
        let kind = CompletionKind::Module;
        let edit = CompletionEdit {
            replace: site.replace_span(),
        };
        let mut completions = Vec::new();
        for candidate in self
            .analysis
            .module_file_candidates(
                self.crate_ref,
                self.file_id,
                site.source().inline_module_path(),
            )
            .context("list module declaration candidates")?
        {
            if site
                .source()
                .declared_children()
                .iter()
                .any(|declared| declared == &candidate)
            {
                continue;
            }
            let label = syntax.identifier(&candidate).to_string();
            completions.push(CompletionItem {
                label: label.clone(),
                filter_text: None,
                kind,
                target,
                applicability: CompletionApplicability::Known,
                detail: Some(definition_detail(kind, &label)),
                documentation: None,
                sort_text: CompletionSortPolicy::General.sort_text(
                    None,
                    &label,
                    kind,
                    CompletionApplicability::Known,
                    target,
                ),
                insert_text: CompletionInsertText::Plain,
                edit: Some(edit),
                additional_edits: Vec::new(),
            });
        }
        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }
}
