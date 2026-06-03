mod completion_site;
mod model;
mod query;
mod source_symbol;

#[cfg(test)]
mod tests;

pub use query::{
    completion::{CompletionClientCapabilities, CompletionQuery},
    references::ReferenceQuery,
};
pub use rg_ir_view::SymbolKind;

use rg_def_map::PackageSlot;
use rg_ir_model::TargetRef;
use rg_ir_view::IndexedViewDb;
use rg_parse::{FileId, TargetId};
use rg_ty::Ty;

use crate::source_symbol::{SourceSymbol, SourceSymbolIndex, SourceSymbolResolver};

pub use self::model::{
    CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem, CompletionKind,
    CompletionTarget, DocumentSymbol, HoverBlock, HoverInfo, KeywordCompletion, NavigationTarget,
    NavigationTargetKind, ReferenceLocation, RenameEdit, RenameResult, RenameTarget, SymbolAt,
    TypeHint, TypePathScopeRef, WorkspaceSymbol,
};

/// High-level LSP-facing query API over one request-scoped project transaction.
pub struct Analysis<'a> {
    view_db: IndexedViewDb<'a>,
}

impl<'a> Analysis<'a> {
    /// Builds a query API over one request-scoped indexed view.
    pub fn new(view_db: IndexedViewDb<'a>) -> Self {
        Self { view_db }
    }

    pub(crate) fn view_db(&self) -> &IndexedViewDb<'a> {
        &self.view_db
    }

    /// Returns the smallest known symbol under a source offset.
    pub fn symbol_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<SymbolAt>> {
        self.symbol_at_for_query(target, file_id, offset)
    }

    pub(crate) fn symbol_at_for_query(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<SymbolAt>> {
        Ok(self
            .source_symbol_at_for_query(target, file_id, offset)?
            .map(SourceSymbol::into_symbol))
    }

    pub(crate) fn source_symbol_at_for_query(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<SourceSymbol>> {
        // Overlapping syntax is common around type paths and expressions. The narrowest span is
        // the best proxy for the thing the user actually placed the cursor on.
        Ok(SourceSymbolIndex::new(self.view_db())
            .symbols_at(target, file_id, offset)?
            .into_iter()
            .min_by_key(|candidate| candidate.span().len()))
    }

    /// Resolves a previously found symbol to navigation targets.
    pub fn resolve_symbol(&self, symbol: SymbolAt) -> anyhow::Result<Vec<NavigationTarget>> {
        query::navigation::SymbolResolver::new(self.view_db()).resolve_symbol(symbol)
    }

    /// Returns best-effort definitions for the symbol under a source offset.
    pub fn goto_definition(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        query::navigation::GotoResolver::new(self).goto_definition(target, file_id, offset)
    }

    /// Returns best-effort type definitions for the symbol under a source offset.
    pub fn goto_type_definition(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        query::navigation::TypeDefinitionResolver::new(self)
            .goto_type_definition(target, file_id, offset)
    }

    /// Returns best-effort implementations for the symbol under a source offset.
    pub fn goto_implementation(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        query::navigation::ImplementationResolver::new(self)
            .goto_implementation(target, file_id, offset)
    }

    /// Returns the best-effort type under a source offset.
    pub fn type_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<Ty>> {
        let Some(symbol) = self.symbol_at_for_query(target, file_id, offset)? else {
            return Ok(None);
        };
        SourceSymbolResolver::new(self.view_db()).ty_for_symbol(symbol)
    }

    /// Returns best-effort inferred type hints for local bindings in one file.
    pub fn type_hints(
        &self,
        target: TargetRef,
        file_id: FileId,
        range: Option<rg_parse::TextSpan>,
    ) -> anyhow::Result<Vec<TypeHint>> {
        query::type_hints::TypeHintCollector::new(self).type_hints(target, file_id, range)
    }

    /// Returns best-effort hover information for the symbol under a source offset.
    pub fn hover(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<HoverInfo>> {
        query::hover::HoverResolver::new(self).hover(target, file_id, offset)
    }

    /// Returns best-effort source references for the symbol under a source offset.
    ///
    /// Only source occurrences inside the query's search surface are scanned. The query also
    /// controls whether declaration locations are included and how they relate to that surface.
    pub fn references(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
        query: ReferenceQuery<'_>,
    ) -> anyhow::Result<Vec<ReferenceLocation>> {
        query::references::ReferenceResolver::new(self, query).references(target, file_id, offset)
    }

    /// Returns the source range and placeholder for a valid rename position.
    pub fn prepare_rename(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<RenameTarget>> {
        query::rename::RenameResolver::new(self).prepare_rename(target, file_id, offset)
    }

    /// Returns semantic source edits for renaming the symbol under a source offset.
    pub fn rename(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
        new_name: &str,
        query: ReferenceQuery<'_>,
    ) -> anyhow::Result<Option<RenameResult>> {
        query::rename::RenameResolver::new(self).rename(target, file_id, offset, new_name, query)
    }

    /// Returns best-effort completion candidates for a source offset.
    ///
    /// The query carries the source position plus editor-local facts, such as live source text
    /// and client snippet support. Recognized sites include member access, paths, lexical names,
    /// record fields, and keywords.
    pub fn completions_at(
        &self,
        query: CompletionQuery<'_>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        query::completion::CompletionResolver::new(self, query).completions_at()
    }

    /// Returns a hierarchical outline for one file under the selected target context.
    pub fn document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
    ) -> anyhow::Result<Vec<DocumentSymbol>> {
        query::symbols::SymbolCollector::new(self).document_symbols(target, file_id)
    }

    /// Returns flat, best-effort symbols matching a case-insensitive workspace query.
    pub fn workspace_symbols(&self, query: &str) -> anyhow::Result<Vec<WorkspaceSymbol>> {
        query::symbols::SymbolCollector::new(self).workspace_symbols(query)
    }

    /// Returns target contexts whose module tree contains a package-local file.
    pub fn targets_for_file(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> anyhow::Result<Vec<TargetRef>> {
        let mut targets = Vec::new();
        let def_map_package = self.view_db().def_map_package(package)?;

        for (target_idx, def_map) in def_map_package.def_maps().iter().enumerate() {
            let target_ref = TargetRef {
                package,
                target: TargetId(target_idx),
            };
            let owns_file = def_map
                .modules()
                .iter()
                .any(|module| module.origin.contains_file(file));
            if owns_file {
                targets.push(target_ref);
            }
        }

        Ok(targets)
    }
}
