//! Transport-neutral editor queries over one frozen project snapshot.
//!
//! `rg_ir_view` exposes reusable semantic and source views over the indexed stores. This crate
//! turns those facts into editor operations: navigation, hover, inlay hints, references, rename,
//! symbols, and completion. Its result models deliberately contain no LSP types, so protocol
//! conversion stays outside the analysis boundary.

mod model;
mod query;
mod source_symbol;

#[cfg(test)]
mod tests;

use std::{collections::HashMap, sync::Arc};

pub use query::{
    code_action::{CodeActionKinds, CodeActionQuery, CodeActionTrigger},
    completion::{CompletionClientCapabilities, CompletionQuery, CompletionSource},
    references::{ReferenceQuery, ReferenceSearchFile, ReferenceSearchLabel},
};
pub use rg_ir_view::SymbolKind;

use anyhow::Context as _;
use rg_ir_model::{CrateRef, PackageSlot};
use rg_ir_view::{IndexedViewDb, source::IndexedModuleFileBase, ty::IndexedType};
use rg_parse::{
    CurrentSource, DeclarationAssociationIndex, DeclarationHeaderCursor, FileId, ModuleFileContext,
    ParseDb, Span,
};
use rg_syntax::SourceFile;

use crate::source_symbol::{SourceSymbol, SourceSymbolIndex, SourceSymbolResolver};

pub use self::model::{
    CodeAction, CodeActionEdit, CodeActionKind, CompletionAdditionalEdit, CompletionApplicability,
    CompletionEdit, CompletionInsertText, CompletionItem, CompletionKind, CompletionTarget,
    DocumentOutline, DocumentSymbol, HoverBlock, HoverInfo, InlayHint, InlayHintKind,
    InlayHintPosition, KeywordCompletion, NavigationTarget, NavigationTargetKind,
    NavigationTargetSource, ReferenceLocation, RenameEdit, RenameResult, RenameTarget, SymbolAt,
    SyntheticCompletionTarget, WorkspaceSymbol,
};

/// Request-scoped façade for editor queries over one frozen project view.
///
/// Most operations start with a file and source offset, then combine semantic facts from
/// `IndexedViewDb` with exact source text from the matching parse snapshot. Results use the
/// transport-neutral models exported by this crate; the LSP layer only converts those models.
pub struct Analysis<'a> {
    view_db: IndexedViewDb<'a>,
    saved_source: SavedSourceView<'a>,
    current_source: Option<CurrentSourceView>,
}

/// One token range proven to name the same declaration in current and saved source.
struct AssociatedSavedHeader {
    current: Span,
    saved: Span,
}

impl AssociatedSavedHeader {
    fn current_span(&self) -> Span {
        self.current
    }

    fn saved_span(&self) -> Span {
        self.saved
    }

    /// Preserve the cursor's position within an associated header token.
    fn saved_offset_for(&self, current_offset: u32) -> u32 {
        let within_token = current_offset
            .saturating_sub(self.current.text.start)
            .min(self.current.len());
        self.saved.text.start + within_token.min(self.saved.len())
    }
}

impl<'a> Analysis<'a> {
    /// Builds a query API over one request-scoped indexed view and its matching source snapshot.
    pub fn new(view_db: IndexedViewDb<'a>, saved_source: SavedSourceView<'a>) -> Self {
        Self {
            view_db,
            saved_source,
            current_source: None,
        }
    }

    /// Attach the request-owned source used to build current Body IR in this analysis.
    pub fn with_current_source(mut self, current_source: CurrentSourceView) -> Self {
        self.current_source = Some(current_source);
        self
    }

    pub(crate) fn view_db(&self) -> &IndexedViewDb<'a> {
        &self.view_db
    }

    pub(crate) fn saved_source_text_for_span(
        &self,
        package: PackageSlot,
        file: FileId,
        span: Span,
    ) -> anyhow::Result<Option<String>> {
        self.saved_source.text_for_span(package, file, span)
    }

    pub(crate) fn saved_source_text_for_file(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> anyhow::Result<Option<String>> {
        self.saved_source.file_text(package, file)
    }

    pub(crate) fn saved_source_line_for_offset(
        &self,
        package: PackageSlot,
        file: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<u32>> {
        self.saved_source.line_for_offset(package, file, offset)
    }

    pub(crate) fn module_file_candidates(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        file_base: IndexedModuleFileBase,
        inline_module_path: &[String],
    ) -> anyhow::Result<Vec<String>> {
        self.saved_source
            .module_file_candidates(crate_ref, file, file_base, inline_module_path)
    }

    pub(crate) fn declared_features(&self, package: PackageSlot) -> &[String] {
        self.saved_source.declared_features(package)
    }

    pub(crate) fn current_source(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> Option<&CurrentSource> {
        let current = self.current_source.as_ref()?;
        current.relationship(package, file)?;
        Some(current.source())
    }

    /// Return how this request's source relates to one saved file interpretation.
    pub fn current_source_relationship(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> Option<SavedSourceRelationship> {
        self.current_source.as_ref()?.relationship(package, file)
    }

    pub(crate) fn declaration_associations(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> Option<&DeclarationAssociationIndex> {
        self.current_source
            .as_ref()?
            .declaration_associations(package, file)
    }

    /// Find the saved header token that represents the current token under the cursor.
    ///
    /// Matching uses the request's ordinary current parse and its prepared declaration
    /// associations. New, renamed, or ambiguous declarations have no safe saved header.
    fn associated_saved_header_at(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        current_offset: u32,
    ) -> anyhow::Result<Option<AssociatedSavedHeader>> {
        let Some(source) = self.current_source(crate_ref.package, file) else {
            return Ok(None);
        };
        let edition = self.view_db.crate_edition(crate_ref)?;
        let Some(syntax) = source.parse(edition).map(|parse| parse.tree()) else {
            return Ok(None);
        };
        let Some(cursor) = DeclarationHeaderCursor::at(&syntax, current_offset) else {
            return Ok(None);
        };
        let Some(saved) = self
            .declaration_associations(crate_ref.package, file)
            .and_then(|associations| associations.saved_header_span(&cursor))
        else {
            return Ok(None);
        };
        Ok(Some(AssociatedSavedHeader {
            current: cursor.current_span(),
            saved,
        }))
    }

    /// Translate a current header-token offset before calling a scanner backed by saved source.
    ///
    /// Equal files use the offset directly. Different files require a unique declaration pairing
    /// and corresponding unchanged header token; other current offsets have no saved coordinate.
    pub(crate) fn saved_header_offset_for_current(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        current_offset: u32,
    ) -> anyhow::Result<Option<u32>> {
        match self.current_source_relationship(crate_ref.package, file) {
            Some(SavedSourceRelationship::Exact) => return Ok(Some(current_offset)),
            Some(SavedSourceRelationship::Different) => {}
            // Analyses without a current-source companion are ordinary saved-source queries.
            None => return Ok(Some(current_offset)),
        }

        let Some(association) = self.associated_saved_header_at(crate_ref, file, current_offset)?
        else {
            return Ok(None);
        };
        Ok(Some(association.saved_offset_for(current_offset)))
    }

    /// Returns the smallest known symbol under a source offset.
    pub fn symbol_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<SymbolAt>> {
        self.symbol_at_for_query(crate_ref, file_id, offset)
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
        // Saved and exact source can share one offset. Edited source is read in four explicit
        // layers: current Body IR, its request-local declaration header, module-level current
        // syntax, then an unchanged header paired to saved semantics. None of those paths
        // interpret the current offset in a saved scanner.
        match self.current_source_relationship(crate_ref.package, file_id) {
            Some(SavedSourceRelationship::Different) => {
                let current_body_symbols = SourceSymbolIndex::new(self.view_db())
                    .body_symbols_at(crate_ref, file_id, offset)?;
                let (body_roots, body_symbols): (Vec<_>, Vec<_>) = current_body_symbols
                    .into_iter()
                    .partition(|symbol| matches!(symbol.symbol(), SymbolAt::FunctionBody { .. }));
                if let Some(symbol) = Self::narrowest_source_symbol(body_symbols) {
                    return Ok(Some(symbol));
                }

                let current_signature_symbols = SourceSymbolIndex::new(self.view_db())
                    .current_signature_symbols_at(crate_ref, file_id, offset)?;
                if let Some(symbol) = Self::narrowest_source_symbol(current_signature_symbols) {
                    return Ok(Some(symbol));
                }

                if let Some(symbol) =
                    self.current_module_use_source_symbol(crate_ref, file_id, offset)?
                {
                    return Ok(Some(symbol));
                }

                // A body root spans its whole declaration so queries can identify the owning
                // function. In a dirty header that broad structural fact must not hide a narrower
                // unchanged token with saved semantics. Keep it only as the final fallback when
                // none of the editor-facing source layers recognize the cursor.
                Ok(self
                    .associated_header_source_symbol(crate_ref, file_id, offset)?
                    .or_else(|| Self::narrowest_source_symbol(body_roots)))
            }
            Some(SavedSourceRelationship::Exact) | None => Ok(Self::narrowest_source_symbol(
                SourceSymbolIndex::new(self.view_db()).symbols_at(crate_ref, file_id, offset)?,
            )),
        }
    }

    /// Resolve a module-level import spelling from current syntax in its matching saved module.
    fn current_module_use_source_symbol(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        current_offset: u32,
    ) -> anyhow::Result<Option<SourceSymbol>> {
        let Some(source) = self.current_source(crate_ref.package, file_id) else {
            return Ok(None);
        };
        let edition = self.view_db.crate_edition(crate_ref)?;
        let Some(parse) = source.parse(edition) else {
            return Ok(None);
        };
        let syntax = parse.tree();
        let symbols = SourceSymbolIndex::new(self.view_db()).current_module_use_symbols_at(
            crate_ref,
            file_id,
            &syntax,
            current_offset,
        )?;
        Ok(Self::narrowest_source_symbol(symbols))
    }

    /// Find saved semantics for a current header token that has one matching saved declaration.
    fn associated_header_source_symbol(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        current_offset: u32,
    ) -> anyhow::Result<Option<SourceSymbol>> {
        let Some(association) =
            self.associated_saved_header_at(crate_ref, file_id, current_offset)?
        else {
            return Ok(None);
        };
        let symbols = SourceSymbolIndex::new(self.view_db()).saved_declaration_symbols_at(
            crate_ref,
            file_id,
            association.saved_span().text.start,
        )?;
        Ok(Self::narrowest_source_symbol(symbols)
            .and_then(|symbol| symbol.for_associated_header(association.current_span())))
    }

    fn narrowest_source_symbol(symbols: Vec<SourceSymbol>) -> Option<SourceSymbol> {
        // Overlapping syntax is common around type paths and expressions. The narrowest span is
        // the best proxy for the thing the user actually placed the cursor on.
        symbols
            .into_iter()
            .min_by_key(|candidate| candidate.span().len())
    }

    /// Resolves a previously found symbol to navigation targets.
    pub fn resolve_symbol(&self, symbol: SymbolAt) -> anyhow::Result<Vec<NavigationTarget>> {
        query::navigation::SymbolResolver::new(self.view_db()).resolve_symbol(symbol)
    }

    /// Returns best-effort definitions for the symbol under a source offset.
    pub fn goto_definition(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        query::navigation::GotoResolver::new(self).goto_definition(crate_ref, file_id, offset)
    }

    /// Returns best-effort type definitions for the symbol under a source offset.
    pub fn goto_type_definition(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        query::navigation::TypeDefinitionResolver::new(self)
            .goto_type_definition(crate_ref, file_id, offset)
    }

    /// Returns best-effort implementations for the symbol under a source offset.
    pub fn goto_implementation(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        query::navigation::ImplementationResolver::new(self)
            .goto_implementation(crate_ref, file_id, offset)
    }

    /// Returns the best-effort type under a source offset.
    pub fn type_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedType>> {
        let Some(symbol) = self.symbol_at_for_query(crate_ref, file_id, offset)? else {
            return Ok(None);
        };
        SourceSymbolResolver::new(self.view_db()).ty_for_symbol(symbol)
    }

    /// Returns best-effort inlay hints for one file.
    pub fn inlay_hints(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        range: Option<rg_parse::TextSpan>,
    ) -> anyhow::Result<Vec<InlayHint>> {
        query::inlay_hints::InlayHintCollector::new(self).inlay_hints(crate_ref, file_id, range)
    }

    /// Returns best-effort hover information for the symbol under a source offset.
    pub fn hover(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<HoverInfo>> {
        query::hover::HoverResolver::new(self).hover(crate_ref, file_id, offset)
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
        query::references::ReferenceResolver::new(self, query)
            .references(crate_ref, file_id, offset)
    }

    /// Returns labels that callers may use for request-local reference prefiltering.
    pub fn reference_search_labels(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<ReferenceSearchLabel>> {
        query::references::ReferenceResolver::reference_search_labels(
            self, crate_ref, file_id, offset,
        )
    }

    /// Returns the source range and placeholder for a valid rename position.
    pub fn prepare_rename(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<RenameTarget>> {
        query::rename::RenameResolver::new(self).prepare_rename(crate_ref, file_id, offset)
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
        query::rename::RenameResolver::new(self).rename(crate_ref, file_id, offset, new_name, query)
    }

    /// Returns best-effort completion candidates for a source offset.
    ///
    /// Semantic sites cover dot members and postfix transforms, qualified and unqualified names,
    /// imports, patterns, record fields, associated type bindings, and missing trait members.
    /// Request-local syntax also supplies keywords, module declarations and macros, attributes,
    /// lifetimes and labels, restricted visibility, const expressions, and recognized
    /// string/macro grammars.
    ///
    /// The query carries the source position plus editor-local facts needed by those providers,
    /// such as the live source buffer and snippet support.
    pub fn completions_at(
        &self,
        query: CompletionQuery<'_>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        query::completion::CompletionResolver::new(self, query).completions_at()
    }

    /// Returns source actions applicable to one range in the captured editor document.
    pub fn code_actions(&self, query: CodeActionQuery<'_>) -> anyhow::Result<Vec<CodeAction>> {
        query::code_action::CodeActionResolver::new(self, query).code_actions()
    }

    /// Returns a hierarchical outline for one file under the selected crate context.
    pub fn document_symbols(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
    ) -> anyhow::Result<DocumentOutline> {
        query::symbols::SymbolCollector::new(self).document_symbols(crate_ref, file_id)
    }

    /// Returns an outline directly from syntax, without requiring a saved file identity.
    pub fn document_symbols_from_syntax(syntax: &SourceFile) -> Vec<DocumentSymbol> {
        query::symbols::SymbolCollector::document_symbols_from_syntax(syntax)
    }

    /// Returns flat, best-effort symbols matching a case-insensitive workspace query.
    pub fn workspace_symbols(&self, query: &str) -> anyhow::Result<Vec<WorkspaceSymbol>> {
        query::symbols::SymbolCollector::new(self).workspace_symbols(query)
    }
}

/// Whether the captured request text is byte-for-byte equal to one saved source interpretation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavedSourceRelationship {
    Exact,
    Different,
}

/// Current request source paired explicitly with each saved file interpretation that may use it.
///
/// The source and its derived syntax are shared across crate interpretations. Each interpretation
/// retains its own equality proof and declaration associations, because saved file identity and
/// semantic ownership remain package-specific.
#[derive(Debug, Clone)]
pub struct CurrentSourceView {
    source: Arc<CurrentSource>,
    interpretations: HashMap<(PackageSlot, FileId), CurrentSourceInterpretation>,
}

impl CurrentSourceView {
    pub fn new(source: Arc<CurrentSource>) -> Self {
        Self {
            source,
            interpretations: HashMap::new(),
        }
    }

    /// Pair this source with one saved file without hiding which coordinate space is in use.
    pub fn add_saved_interpretation(
        &mut self,
        package: PackageSlot,
        file: FileId,
        relationship: SavedSourceRelationship,
        associations: Arc<DeclarationAssociationIndex>,
    ) {
        self.interpretations.insert(
            (package, file),
            CurrentSourceInterpretation {
                relationship,
                associations,
            },
        );
    }

    /// Return the immutable editor source shared by every saved interpretation in this view.
    pub fn source(&self) -> &CurrentSource {
        &self.source
    }

    /// Clone the shared source handle without copying its text or parsed syntax trees.
    pub fn shared_source(&self) -> Arc<CurrentSource> {
        Arc::clone(&self.source)
    }

    /// Tell whether the editor text is byte-for-byte equal to this saved file interpretation.
    pub fn relationship(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> Option<SavedSourceRelationship> {
        Some(self.interpretations.get(&(package, file))?.relationship)
    }

    /// Return the conservative declaration pairs prepared for this saved interpretation.
    pub fn declaration_associations(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> Option<&DeclarationAssociationIndex> {
        Some(&self.interpretations.get(&(package, file))?.associations)
    }
}

#[derive(Debug, Clone)]
struct CurrentSourceInterpretation {
    relationship: SavedSourceRelationship,
    associations: Arc<DeclarationAssociationIndex>,
}

/// Saved-source companion to the indexed facts used by `Analysis`.
///
/// Editor queries need facts that are intentionally absent from semantic indexes: exact source
/// slices for rename and inlay labels, full files for edit planning, line positions, declared
/// Cargo features, and conventional sibling files for `mod name;`. Keeping that access here lets
/// `rg_ir_view` stay focused on reusable indexed facts.
#[derive(Debug, Clone, Copy)]
pub struct SavedSourceView<'a> {
    parse: &'a ParseDb,
}

impl<'a> SavedSourceView<'a> {
    pub fn new(parse: &'a ParseDb) -> Self {
        Self { parse }
    }

    fn declared_features(&self, package: PackageSlot) -> &[String] {
        self.parse
            .package(package.0)
            .map(rg_parse::Package::declared_features)
            .unwrap_or_default()
    }

    /// Lists conventional out-of-line child modules below one file-backed source position.
    fn module_file_candidates(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        file_base: IndexedModuleFileBase,
        inline_module_path: &[String],
    ) -> anyhow::Result<Vec<String>> {
        let Some(parsed_file) = self
            .parse
            .package(crate_ref.package.0)
            .and_then(|package| package.parsed_file(file))
        else {
            return Ok(Vec::new());
        };

        let mut context = match file_base {
            IndexedModuleFileBase::TargetRoot => {
                ModuleFileContext::for_target_root(parsed_file.path())
            }
            IndexedModuleFileBase::Conventional => {
                ModuleFileContext::from_definition_file(parsed_file.path())
            }
            IndexedModuleFileBase::PathAttribute => {
                ModuleFileContext::for_path_attribute_file(parsed_file.path())
            }
        };
        // TODO: Carry direct `#[path]` overrides on inline-module ancestors in the indexed source
        // site. Their directory cannot be reconstructed from the semantic name alone.
        for module_name in inline_module_path {
            context = context.descend_inline(module_name, None);
        }
        context
            .candidate_module_names()
            .context("list module declaration completion candidates")
    }

    fn text_for_span(
        &self,
        package: PackageSlot,
        file: FileId,
        span: Span,
    ) -> anyhow::Result<Option<String>> {
        let Some(parsed_file) = self
            .parse
            .package(package.0)
            .and_then(|package| package.parsed_file(file))
        else {
            return Ok(None);
        };
        parsed_file.text_for_span(span)
    }

    fn file_text(&self, package: PackageSlot, file: FileId) -> anyhow::Result<Option<String>> {
        let Some(parsed_file) = self
            .parse
            .package(package.0)
            .and_then(|package| package.parsed_file(file))
        else {
            return Ok(None);
        };
        let text = parsed_file
            .source_text()
            .context("load completion source file text")?;
        Ok(Some(text.to_string()))
    }

    fn line_for_offset(
        &self,
        package: PackageSlot,
        file: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<u32>> {
        let Some(parsed_file) = self
            .parse
            .package(package.0)
            .and_then(|package| package.parsed_file(file))
        else {
            return Ok(None);
        };
        Ok(Some(parsed_file.line_index()?.position(offset).line))
    }
}
