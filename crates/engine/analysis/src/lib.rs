use rg_body_ir::{BodyIrReadTxn, BodyTy};
use rg_def_map::{DefMapReadTxn, PackageSlot, TargetRef};
use rg_parse::FileId;
use rg_semantic_ir::SemanticIrReadTxn;

mod completion;
mod cursor;
mod data;
mod entity;
mod hints;
mod hover;
mod navigation;
mod path_render;
mod signature;
mod symbol;
mod symbols;
mod txn;
mod ty;
mod type_render;

#[cfg(test)]
mod tests;

pub use self::data::{
    CompletionApplicability, CompletionItem, DocumentSymbol, HoverBlock, HoverInfo,
    NavigationTarget, SymbolAt, TypeHint, WorkspaceSymbol,
};
pub use self::data::{CompletionKind, CompletionTarget, NavigationTargetKind, SymbolKind};
pub use self::txn::AnalysisReadTxn;

/// High-level LSP-facing query API over one request-scoped project transaction.
pub struct Analysis<'a> {
    def_map: DefMapReadTxn<'a>,
    semantic_ir: SemanticIrReadTxn<'a>,
    body_ir: BodyIrReadTxn<'a>,
}

impl<'a> Analysis<'a> {
    /// Builds a query API over one request-scoped analysis transaction.
    ///
    /// # Safety
    ///
    /// The transaction must contain phase transactions from the same immutable project snapshot and
    /// package subset. Mixing phases from different project revisions can pair semantic facts with
    /// unrelated source files, package slots, or line indexes.
    pub fn new(txn: &AnalysisReadTxn<'a>) -> Self {
        Self {
            def_map: txn.def_map().clone(),
            semantic_ir: txn.semantic_ir().clone(),
            body_ir: txn.body_ir().clone(),
        }
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
        symbol::SymbolFinder::new(self).symbol_at(target, file_id, offset)
    }

    /// Resolves a previously found symbol to navigation targets.
    pub fn resolve_symbol(&self, symbol: SymbolAt) -> anyhow::Result<Vec<NavigationTarget>> {
        navigation::SymbolResolver::new(self).resolve_symbol(symbol)
    }

    /// Returns best-effort definitions for the symbol under a source offset.
    pub fn goto_definition(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        navigation::GotoResolver::new(self).goto_definition(target, file_id, offset)
    }

    /// Returns best-effort type definitions for the symbol under a source offset.
    pub fn goto_type_definition(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        navigation::TypeDefinitionResolver::new(self).goto_type_definition(target, file_id, offset)
    }

    /// Returns the best-effort Body IR type under a source offset.
    pub fn type_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<BodyTy>> {
        ty::TypeResolver::new(self).type_at(target, file_id, offset)
    }

    /// Returns best-effort inferred type hints for local bindings in one file.
    pub fn type_hints(
        &self,
        target: TargetRef,
        file_id: FileId,
        range: Option<rg_parse::TextSpan>,
    ) -> anyhow::Result<Vec<TypeHint>> {
        hints::TypeHintCollector::new(self).type_hints(target, file_id, range)
    }

    /// Returns best-effort hover information for the symbol under a source offset.
    pub fn hover(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<HoverInfo>> {
        hover::HoverResolver::new(self).hover(target, file_id, offset)
    }

    /// Returns field and method completion candidates for a receiver before a dot.
    pub fn completions_at_dot(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        completion::CompletionResolver::new(self).completions_at_dot(target, file_id, offset)
    }

    /// Returns a hierarchical outline for one file under the selected target context.
    pub fn document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
    ) -> anyhow::Result<Vec<DocumentSymbol>> {
        symbols::SymbolCollector::new(self).document_symbols(target, file_id)
    }

    /// Returns flat, best-effort symbols matching a case-insensitive workspace query.
    pub fn workspace_symbols(&self, query: &str) -> anyhow::Result<Vec<WorkspaceSymbol>> {
        symbols::SymbolCollector::new(self).workspace_symbols(query)
    }

    /// Returns target contexts whose module tree contains a package-local file.
    pub fn targets_for_file(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> anyhow::Result<Vec<TargetRef>> {
        let mut targets = Vec::new();
        let def_map_package = self.def_map.package(package)?;

        for (target_idx, def_map) in def_map_package.into_ref().targets().iter().enumerate() {
            let target_ref = TargetRef {
                package,
                target: rg_parse::TargetId(target_idx),
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
