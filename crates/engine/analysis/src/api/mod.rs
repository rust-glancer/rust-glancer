mod completion_site;
mod query;
mod render;
mod source_symbol;
mod view;

pub use query::{
    completion::{CompletionClientCapabilities, CompletionQuery},
    references::ReferenceQuery,
};

use rg_body_ir::BodyIrReadTxn;
use rg_def_map::{DefMapReadTxn, PackageSlot};
use rg_ir_model::TargetRef;
use rg_parse::FileId;
use rg_semantic_ir::SemanticIrReadTxn;
use rg_ty::IndexedTy;

use crate::{
    api::source_symbol::{SourceSymbol, SourceSymbolIndex},
    model::{
        CompletionItem, DeclarationRef, DocumentSymbol, HoverInfo, NavigationTarget,
        ReferenceLocation, SymbolAt, TypeHint, TypePathScopeRepr, WorkspaceSymbol,
    },
    txn::AnalysisReadTxn,
};

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
        Ok(SourceSymbolIndex::new(self)
            .symbols_at(target, file_id, offset)?
            .into_iter()
            .min_by_key(|candidate| candidate.span().len()))
    }

    pub(crate) fn declarations_for_source_symbol(
        &self,
        symbol: SymbolAt,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let resolution = view::resolution::ResolutionView::new(self);
        match symbol {
            SymbolAt::FunctionBody { .. } => Ok(Vec::new()),
            SymbolAt::Declaration { declaration, .. } => {
                resolution.declarations_for_declaration(declaration)
            }
            SymbolAt::Expr { expr } => {
                let body = expr.body_ir();
                let Some(body_data) = self.body_ir.body_data(body)? else {
                    return Ok(Vec::new());
                };
                let Some(expr_data) = body_data.expr(expr.expr_id()) else {
                    return Ok(Vec::new());
                };
                resolution.declarations_for_body_resolution(Some(body), &expr_data.resolution)
            }
            SymbolAt::TypePath { scope, path, .. } => match scope.repr() {
                TypePathScopeRepr::Signature(context) => {
                    let declarations =
                        resolution.declarations_for_semantic_type_path(context, &path)?;
                    if declarations.is_empty() {
                        resolution.declarations_for_use_path(context.module, &path)
                    } else {
                        Ok(declarations)
                    }
                }
                TypePathScopeRepr::Body(scope) => resolution.declarations_for_body_type_path(
                    scope.body_ir(),
                    scope.scope_id(),
                    &path,
                ),
            },
            SymbolAt::ValuePath { scope, path, .. } => resolution.declarations_for_body_value_path(
                scope.body_ir(),
                scope.scope_id(),
                &path,
            ),
            SymbolAt::UsePath { module, path, .. } => {
                resolution.declarations_for_use_path(module, &path)
            }
        }
    }

    pub(crate) fn ty_for_source_symbol(
        &self,
        symbol: SymbolAt,
    ) -> anyhow::Result<Option<IndexedTy>> {
        let ty_view = view::ty::TyView::new(self);
        let ty = match symbol {
            SymbolAt::Expr { expr } => ty_view.ty_for_expr(expr)?,
            SymbolAt::Declaration { declaration, .. } => {
                let mut ty = None;
                for declaration in view::resolution::ResolutionView::new(self)
                    .declarations_for_declaration(declaration)?
                {
                    if let Some(declaration_ty) = ty_view.ty_for_declaration(declaration)? {
                        ty = Some(declaration_ty);
                        break;
                    }
                }
                ty
            }
            SymbolAt::TypePath { scope, path, .. } => match scope.repr() {
                TypePathScopeRepr::Signature(context) => {
                    Some(ty_view.ty_for_type_path(context, &path)?)
                }
                TypePathScopeRepr::Body(scope) => {
                    Some(ty_view.ty_for_body_type_path(scope.body_ir(), scope.scope_id(), &path)?)
                }
            },
            SymbolAt::ValuePath { scope, path, .. } => {
                Some(ty_view.ty_for_body_value_path(scope.body_ir(), scope.scope_id(), &path)?)
            }
            SymbolAt::UsePath { .. } | SymbolAt::FunctionBody { .. } => None,
        };
        Ok(ty)
    }

    /// Resolves a previously found symbol to navigation targets.
    pub fn resolve_symbol(&self, symbol: SymbolAt) -> anyhow::Result<Vec<NavigationTarget>> {
        query::navigation::SymbolResolver::new(self).resolve_symbol(symbol)
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

    /// Returns the best-effort indexed type under a source offset.
    pub fn type_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedTy>> {
        let Some(symbol) = self.symbol_at_for_query(target, file_id, offset)? else {
            return Ok(None);
        };
        self.ty_for_source_symbol(symbol)
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
