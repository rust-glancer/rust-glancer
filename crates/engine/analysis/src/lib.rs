//! Transport-neutral editor queries over one frozen project snapshot.
//!
//! `rg_ir_view` exposes reusable semantic and source views over the indexed stores. This crate
//! turns those facts into editor operations: navigation, hover, inlay hints, references, rename,
//! symbols, and completion. Its result models deliberately contain no LSP types, so protocol
//! conversion stays outside the analysis boundary.

mod documentation;
mod model;
mod query;
mod source_symbol;

#[cfg(test)]
mod tests;

use anyhow::Context as _;
use rg_ir_model::{CrateRef, FileId, PackageSlot, Span};
pub use rg_ir_view::SymbolKind;
use rg_ir_view::{IndexedViewDb, source::IndexedSourceProvenance, ty::IndexedType};
use rg_parse::SourceReader;
use rg_std::{Cancelable, Cancelled};
use rg_syntax::SourceFile;
use rg_workspace::WorkspaceMetadata;

pub use self::{
    model::{
        CodeAction, CodeActionEdit, CodeActionKind, CompletionAdditionalEdit,
        CompletionApplicability, CompletionEdit, CompletionImport, CompletionInsertText,
        CompletionItem, CompletionKind, CompletionResult, CompletionTarget, DocumentSymbol,
        DocumentationLink, Fold, FoldKind, Highlight, HighlightKind, HoverBlock, HoverInfo,
        InlayHint, InlayHintKind, InlayHintPosition, KeywordCompletion, NavigationTarget,
        NavigationTargetKind, ReferenceLocation, RenameEdit, RenameResult, RenameTarget, SymbolAt,
        SyntheticCompletionTarget, WorkspaceSymbol,
    },
    query::{
        code_action::{CodeActionKinds, CodeActionQuery, CodeActionTrigger},
        completion::{CompletionClientCapabilities, CompletionQuery, CompletionSource},
        references::{
            ReferenceQuery, ReferenceSearchFile, ReferenceSearchKind, ReferenceSearchLabel,
        },
    },
};
use crate::source_symbol::{SourceSymbol, SourceSymbolIndex, SourceSymbolResolver};

/// Request-scoped façade for editor queries over one frozen project view.
///
/// Most operations start with a file and source offset, then combine semantic facts from
/// `IndexedViewDb` with source text. Results use the transport-neutral models exported by this
/// crate; the LSP layer only converts those models.
pub struct Analysis<'a> {
    view_db: IndexedViewDb<'a>,
    sources: SourceReader<'a>,
    workspace: &'a WorkspaceMetadata,
}

impl Cancelable for Analysis<'_> {
    fn check_cancelled(&self, checkpoint: &'static str) -> Result<(), Cancelled> {
        self.view_db.check_cancelled(checkpoint)
    }
}

impl<'a> Analysis<'a> {
    /// Highlight names and formatting punctuation in strings passed to formatting builtins.
    /// Macro expansion handles aliases and wrappers; no body analysis is needed.
    pub fn format_string_highlights(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        range: Option<Span>,
    ) -> anyhow::Result<Vec<Highlight>> {
        self.run_query("format string highlighting", || {
            query::highlight::FormatStringHighlighter::new(self).highlight(crate_ref, file, range)
        })
    }

    /// Highlight documentation and resolve its links.
    /// Examples need syntax only; this operation does not require body analysis.
    pub fn documentation_highlights(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        range: Option<Span>,
    ) -> anyhow::Result<Vec<Highlight>> {
        self.run_query("documentation highlighting", || {
            documentation::DocumentationHighlighter::new(self).highlight(crate_ref, file, range)
        })
    }

    pub fn new(view_db: IndexedViewDb<'a>, workspace: &'a WorkspaceMetadata) -> Self {
        Self {
            sources: view_db.sources(),
            view_db,
            workspace,
        }
    }

    pub(crate) fn view_db(&self) -> &IndexedViewDb<'a> {
        &self.view_db
    }

    pub(crate) fn source_syntax(
        &self,
        crate_ref: CrateRef,
        file: FileId,
    ) -> anyhow::Result<SourceFile> {
        let edition = self.view_db().crate_edition(crate_ref)?;
        Ok(self.sources.syntax(file, edition)?.tree())
    }

    /// Kernels may finish their last unit as cancellation arrives. Check both sides of the
    /// operation so a fail-soft solver exit can never escape as an empty or partial query result.
    fn run_query<T>(
        &self,
        name: &'static str,
        query: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        rg_std::check_cancel!(self, name);
        let result = query().context("execute semantic query")?;
        rg_std::check_cancel!(self, name);
        Ok(result)
    }

    pub(crate) fn declared_features(&self, package: PackageSlot) -> &[String] {
        self.workspace
            .packages()
            .get(package.0)
            .map(|package| package.declared_features.as_slice())
            .unwrap_or_default()
    }

    /// Returns the smallest known symbol under a source offset.
    pub fn symbol_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<SymbolAt>> {
        self.run_query("symbol_at", || {
            self.symbol_at_for_query(crate_ref, file_id, offset)
        })
    }

    pub(crate) fn symbol_at_for_query(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<SymbolAt>> {
        Ok(self
            .source_symbol_at_for_query(crate_ref, file_id, offset)?
            .map(SourceSymbol::into_symbol))
    }

    pub(crate) fn source_symbol_at_for_query(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<SourceSymbol>> {
        Ok(self
            .source_symbols_at_for_query(crate_ref, file_id, offset)?
            .into_iter()
            .next())
    }

    /// Return all expanded uses represented by the selected macro argument token.
    ///
    /// A macro can turn `both!(run)` into `(First.run(), Second.run())`. Both calls share `run`'s
    /// written span, but each can name a different method.
    /// Ordinary source keeps its existing single-symbol selection policy; expansion repetitions
    /// must not silently select the first inferred type or declaration.
    pub(crate) fn source_symbols_at_for_query(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        Ok(Self::narrowest_source_symbols(
            SourceSymbolIndex::new(self.view_db()).symbols_at(crate_ref, file_id, offset)?,
        ))
    }

    fn narrowest_source_symbols(mut symbols: Vec<SourceSymbol>) -> Vec<SourceSymbol> {
        // Overlapping syntax is common around type paths and expressions. Keep the first candidate
        // at the narrowest span, unless it is macro input with several expanded uses of that token.
        let Some((selected_index, selected)) = symbols
            .iter()
            .enumerate()
            .min_by_key(|(_, candidate)| candidate.span().len())
        else {
            return symbols;
        };
        if !matches!(
            selected.provenance(),
            IndexedSourceProvenance::MacroArgument { .. }
        ) {
            return vec![symbols.swap_remove(selected_index)];
        }
        let span = selected.span();
        symbols.retain(|symbol| {
            symbol.span() == span
                && matches!(
                    symbol.provenance(),
                    IndexedSourceProvenance::MacroArgument { .. }
                )
        });
        symbols
    }

    /// Resolves a previously found symbol to navigation targets.
    pub fn resolve_symbol(&self, symbol: SymbolAt) -> anyhow::Result<Vec<NavigationTarget>> {
        self.run_query("resolve_symbol", || {
            query::navigation::SymbolResolver::new(self.view_db()).resolve_symbol(symbol)
        })
    }

    /// Returns best-effort definitions for the symbol under a source offset.
    pub fn goto_definition(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        self.run_query("goto_definition", || {
            query::navigation::GotoResolver::new(self).goto_definition(crate_ref, file_id, offset)
        })
    }

    /// Returns best-effort type definitions for the symbol under a source offset.
    pub fn goto_type_definition(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        self.run_query("goto_type_definition", || {
            query::navigation::TypeDefinitionResolver::new(self)
                .goto_type_definition(crate_ref, file_id, offset)
        })
    }

    /// Returns best-effort implementations for the symbol under a source offset.
    pub fn goto_implementation(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        self.run_query("goto_implementation", || {
            query::navigation::ImplementationResolver::new(self)
                .goto_implementation(crate_ref, file_id, offset)
        })
    }

    /// Returns the best-effort type under a source offset.
    pub fn type_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedType>> {
        self.run_query("type_at", || {
            let Some(symbol) = self.symbol_at_for_query(crate_ref, file_id, offset)? else {
                return Ok(None);
            };
            SourceSymbolResolver::new(self.view_db()).ty_for_symbol(symbol)
        })
    }

    /// Returns best-effort inlay hints in the requested source range.
    /// `max_length` is a soft character budget per type label. `None` skips length-based
    /// shortening; iterator simplification still applies, and tooltips retain concrete text.
    pub fn inlay_hints(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        range: Span,
        max_length: Option<usize>,
    ) -> anyhow::Result<Vec<InlayHint>> {
        self.run_query("inlay_hints", || {
            query::inlay_hints::InlayHintCollector::new(self, max_length)
                .inlay_hints(crate_ref, file_id, range)
        })
    }

    /// Returns best-effort hover information for the symbol under a source offset.
    pub fn hover(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<HoverInfo>> {
        self.run_query("hover", || {
            query::hover::HoverResolver::new(self).hover(crate_ref, file_id, offset)
        })
    }

    /// Returns best-effort source references for the symbol under a source offset.
    ///
    /// Only source occurrences inside the query's search surface are scanned. The query also
    /// controls whether declaration locations are included and how they relate to that surface.
    pub fn references(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        query: ReferenceQuery<'_>,
    ) -> anyhow::Result<Vec<ReferenceLocation>> {
        self.run_query("references", || {
            query::references::ReferenceResolver::new(self, query)
                .references(crate_ref, file_id, offset)
        })
    }

    /// Returns labels that callers may use for request-local reference prefiltering.
    pub fn reference_search_labels(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        kind: ReferenceSearchKind,
    ) -> anyhow::Result<Vec<ReferenceSearchLabel>> {
        self.run_query("reference_search_labels", || {
            query::references::ReferenceResolver::reference_search_labels(
                self, crate_ref, file_id, offset, kind,
            )
        })
    }

    /// Returns the source range and placeholder for a valid rename position.
    pub fn prepare_rename(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<RenameTarget>> {
        self.run_query("prepare_rename", || {
            query::rename::RenameResolver::new(self).prepare_rename(crate_ref, file_id, offset)
        })
    }

    /// Returns semantic source edits for renaming the symbol under a source offset.
    pub fn rename(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        new_name: &str,
        query: ReferenceQuery<'_>,
    ) -> anyhow::Result<Option<RenameResult>> {
        self.run_query("rename", || {
            query::rename::RenameResolver::new(self)
                .rename(crate_ref, file_id, offset, new_name, query)
        })
    }

    /// Returns best-effort completion candidates for a source offset.
    ///
    /// Semantic sites cover dot members and postfix transforms, qualified and unqualified names,
    /// imports, patterns, record fields, associated type bindings, and missing trait members.
    /// Request-local syntax also supplies keywords, module declarations and macros, attributes,
    /// lifetimes and labels, restricted visibility, const expressions, and recognized
    /// string/macro grammars.
    ///
    /// Missing source text comes from this analysis snapshot. The caller can also supply its
    /// captured text or reuse speculative syntax already prepared for the request.
    pub fn completions_at(&self, query: CompletionQuery<'_>) -> anyhow::Result<CompletionResult> {
        self.run_query("completions_at", || {
            let snapshot_text;
            let source_text = match query.source_text {
                Some(text) => text,
                None => {
                    snapshot_text = self.sources.text(query.file_id)?;
                    &snapshot_text
                }
            };
            query::completion::CompletionResolver::new(self, query, source_text).completions_at()
        })
    }

    /// Plan the source edit for a selected auto-import completion.
    ///
    /// For an older item, recheck the selected name or method against the current scope.
    pub fn resolve_completion_import(
        &self,
        query: CompletionQuery<'_>,
        import: &CompletionImport,
        selected: Option<(&str, CompletionKind)>,
    ) -> anyhow::Result<Option<CompletionAdditionalEdit>> {
        self.run_query("resolve_completion_import", || {
            let snapshot_text;
            let source = match query.source_text {
                Some(text) => text,
                None => {
                    snapshot_text = self.sources.text(query.file_id)?;
                    &snapshot_text
                }
            };
            query::completion::resolve_import(self, query, source, import, selected)
        })
    }

    /// Returns source actions applicable to one range in the captured editor document.
    pub fn code_actions(&self, query: CodeActionQuery<'_>) -> anyhow::Result<Vec<CodeAction>> {
        self.run_query("code_actions", || {
            query::code_action::CodeActionResolver::new(self, query).code_actions()
        })
    }

    /// Returns an outline directly from syntax, without requiring a file identity.
    pub fn document_symbols_from_syntax(syntax: &SourceFile) -> Vec<DocumentSymbol> {
        query::symbols::SymbolCollector::document_symbols_from_syntax(syntax)
    }

    /// Returns folding ranges directly from syntax, without requiring a file identity.
    pub fn folding_ranges_from_syntax(syntax: &SourceFile) -> Vec<Fold> {
        query::folding::SyntaxFoldCollector::collect(syntax)
    }

    /// Returns flat, best-effort symbols matching a case-insensitive workspace query.
    pub fn workspace_symbols(&self, query: &str) -> anyhow::Result<Vec<WorkspaceSymbol>> {
        self.run_query("workspace_symbols", || {
            query::symbols::SymbolCollector::new(self).workspace_symbols(query)
        })
    }
}
