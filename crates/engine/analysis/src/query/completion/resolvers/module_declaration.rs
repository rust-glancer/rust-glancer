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
use rg_ir_model::{CrateRef, FileId};
use rg_ir_view::{display::syntax::SyntaxRenderer, source::IndexedModuleFileBase};
use rg_parse::ModuleFileContext;

use crate::{
    Analysis,
    model::{
        CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem,
        CompletionKind, CompletionTarget, SyntheticCompletionTarget,
    },
    query::completion::{
        render::{CompletionSortPolicy, definition_detail},
        site::ModuleDeclarationCompletionSite,
    },
};

/// Discovers conventional sibling module files and renders their stem as `mod name` candidates.
pub(super) struct ModuleDeclarationCompletionResolver<'a, 'db> {
    analysis: &'a Analysis<'db>,
    crate_ref: CrateRef,
    file_id: FileId,
}

impl<'a, 'db> ModuleDeclarationCompletionResolver<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>, crate_ref: CrateRef, file_id: FileId) -> Self {
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
        // Resolve child filenames relative to the declaration's module context. Source paths
        // need no crate lookup: the selected site already tells us how Rust interprets the file.
        let path = self.analysis.sources.path(self.file_id)?;
        let mut context = match site.source().file_base() {
            IndexedModuleFileBase::TargetRoot => ModuleFileContext::for_target_root(path),
            IndexedModuleFileBase::Conventional => ModuleFileContext::from_definition_file(path),
            IndexedModuleFileBase::PathAttribute => {
                ModuleFileContext::for_path_attribute_file(path)
            }
        };
        // TODO: Carry direct `#[path]` overrides on inline-module ancestors in the indexed source
        // site. Their directory cannot be reconstructed from the semantic name alone.
        for module_name in site.source().inline_module_path() {
            context = context.descend_inline(module_name, None);
        }
        let mut completions = Vec::new();
        for candidate in context
            .candidate_module_names()
            .context("list module declaration candidates")?
        {
            rg_std::check_cancel!(self.analysis, "completion candidate");
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
                import: None,
            });
        }
        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }
}
