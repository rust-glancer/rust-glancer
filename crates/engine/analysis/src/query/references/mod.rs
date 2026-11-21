//! Reference searches discover plausible source owners before demanding semantic facts.
//!
//! The existing source scanners prove each candidate without a separate reference index.
//! The query owns the search surface, declaration-inclusion policy, and declaration projection.

use std::collections::HashSet;

use rg_ir_model::{CrateRef, FileId, identity::DeclarationRef};
use rg_ir_view::IndexedViewDb;
use rg_std::UniqueVec;

use crate::{
    Analysis,
    model::{ReferenceLocation, SymbolAt},
    source_symbol::{SourceSymbol, SourceSymbolIndex, SourceSymbolResolver, SourceSymbolRole},
};

mod search;
mod subject;

pub use rg_ir_view::source::ReferenceSearchLabel;

pub use self::search::{ReferenceQuery, ReferenceSearchFile, ReferenceSearchKind};
use self::{
    search::{ReferenceScanTarget, ReferenceSearchScope},
    subject::{ReferenceSearchHints, ReferenceSubject},
};

pub(crate) struct ReferenceResolver<'a, 'db, 'scope> {
    analysis: &'a Analysis<'db>,
    query: ReferenceQuery<'scope>,
}

impl<'a, 'db, 'scope> ReferenceResolver<'a, 'db, 'scope> {
    pub(crate) fn new(analysis: &'a Analysis<'db>, query: ReferenceQuery<'scope>) -> Self {
        Self { analysis, query }
    }

    /// Returns declaration spellings used to narrow the request's source scan.
    pub(crate) fn reference_search_labels(
        analysis: &Analysis<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        kind: ReferenceSearchKind,
    ) -> anyhow::Result<Vec<ReferenceSearchLabel>> {
        let mut declarations = UniqueVec::new();
        for symbol in analysis.source_symbols_at_for_query(crate_ref, file_id, offset)? {
            for declaration in
                Self::unique_declarations_for_analysis(analysis, symbol.into_symbol())?
            {
                declarations.push(declaration);
            }
        }
        let declarations = declarations.into_vec();
        let (_, hints) = ReferenceSubject::resolve(analysis.view_db(), &declarations, kind)?;
        if hints.local_owners().is_some() {
            return Ok(Vec::new());
        }
        Ok(hints.selection.labels)
    }

    /// Finds references for the symbol under `offset` by scanning the requested use-site surface.
    ///
    /// Declaration locations are projected from the selected symbol before use-site scanning when
    /// requested, using the resolver's declaration scope policy.
    pub(crate) fn references(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<ReferenceLocation>> {
        let symbols = self.matching_source_symbols(crate_ref, file_id, offset)?;
        let mut locations = symbols
            .into_iter()
            .map(|symbol| ReferenceLocation {
                crate_ref: symbol.crate_ref(),
                file_id: symbol.file_id(),
                span: symbol.span(),
            })
            .collect::<Vec<_>>();

        locations.sort_by_key(|location| {
            (
                location.crate_ref.package.0,
                location.crate_ref.crate_id.0,
                location.file_id,
                location.span.start,
                location.span.end,
            )
        });
        locations.dedup();
        Ok(locations)
    }

    pub(crate) fn matching_source_symbols(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        let cursor_symbols = self
            .analysis
            .source_symbols_at_for_query(crate_ref, file_id, offset)?;
        let mut declarations = UniqueVec::new();
        for symbol in &cursor_symbols {
            for declaration in self.unique_declarations_for_symbol(symbol.symbol().clone())? {
                declarations.push(declaration);
            }
        }
        if declarations.is_empty() {
            return Ok(Vec::new());
        }
        let mut symbols = self.source_symbols_matching_declarations(
            &declarations.into_vec(),
            ReferenceSearchKind::FindReferences,
        )?;
        for symbol in cursor_symbols {
            let included = match symbol.role() {
                SourceSymbolRole::Reference => {
                    self.query.accepts_scan_target(ReferenceScanTarget {
                        crate_ref: symbol.crate_ref(),
                        file_id: Some(symbol.file_id()),
                    })
                }
                SourceSymbolRole::Declaration => {
                    self.query.includes_declarations()
                        && self
                            .query
                            .accepts_declaration(symbol.crate_ref(), symbol.file_id())
                }
                SourceSymbolRole::Structural => false,
            };
            // Keep an already-proven cursor use even when an import alias gives it a spelling
            // different from the declaration. Each expansion interpretation contributes independently.
            if included && !symbols.contains(&symbol) {
                symbols.push(symbol);
            }
        }
        Ok(symbols)
    }

    pub(crate) fn source_symbols_matching_declarations(
        &self,
        declarations: &[DeclarationRef],
        kind: ReferenceSearchKind,
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        if declarations.is_empty() {
            return Ok(Vec::new());
        }

        let (subject, hints) =
            ReferenceSubject::resolve(self.analysis.view_db(), declarations, kind)?;
        self.source_symbols_matching_subject(&subject, &hints)
    }

    fn source_symbols_matching_subject(
        &self,
        subject: &ReferenceSubject,
        hints: &ReferenceSearchHints,
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        let matcher = ReferenceDeclarationMatcher::new(self.analysis.view_db(), subject);
        let mut symbols = Vec::new();
        self.push_matching_reference_candidates(&matcher, hints, &mut symbols)?;

        // Rename needs declaration occurrences with their source-surface metadata. Prefer scanned
        // source symbols when they exist, and project a plain declaration only for declarations
        // outside the requested scan surface.
        if self.query.includes_declarations() {
            for location in subject.declaration_locations() {
                rg_std::check_cancel!(self.analysis, "reference declarations");
                if !self
                    .query
                    .accepts_declaration(location.crate_ref, location.file_id)
                {
                    continue;
                }
                if symbols.iter().any(|symbol| {
                    symbol.role() == SourceSymbolRole::Declaration
                        && symbol.crate_ref() == location.crate_ref
                        && symbol.file_id() == location.file_id
                        && symbol.span() == location.location.span
                }) {
                    continue;
                }
                symbols.push(SourceSymbol::plain_declaration(
                    location.declaration,
                    location.crate_ref,
                    location.file_id,
                    location.location,
                ));
            }
        }

        symbols.sort_by_key(|symbol| {
            (
                symbol.crate_ref().package.0,
                symbol.crate_ref().crate_id.0,
                symbol.file_id(),
                symbol.span().start,
                symbol.span().end,
            )
        });
        symbols.dedup();
        Ok(symbols)
    }

    fn unique_declarations_for_symbol(
        &self,
        symbol: SymbolAt,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        Self::unique_declarations_for_analysis(self.analysis, symbol)
    }

    fn unique_declarations_for_analysis(
        analysis: &Analysis<'db>,
        symbol: SymbolAt,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let declarations =
            SourceSymbolResolver::new(analysis.view_db()).declarations_for_symbol(symbol)?;
        let mut unique = UniqueVec::new();
        for declaration in declarations {
            unique.push(declaration);
        }
        Ok(unique.into_vec())
    }

    fn push_matching_reference_candidates(
        &self,
        matcher: &ReferenceDeclarationMatcher<'_, 'db>,
        hints: &ReferenceSearchHints,
        symbols: &mut Vec<SourceSymbol>,
    ) -> anyhow::Result<()> {
        let mut visited = Vec::new();

        if let Some(owners) = hints.local_owners() {
            let index = SourceSymbolIndex::new(self.analysis.view_db());
            for &owner in owners {
                for candidate in index.symbols_in_body(owner)? {
                    // Includes can put occurrences outside the owner's declaration file. Apply
                    // the user's physical-file restriction to results, not to body acquisition.
                    let scan = ReferenceScanTarget {
                        crate_ref: candidate.crate_ref(),
                        file_id: Some(candidate.file_id()),
                    };
                    if self.query.accepts_scan_target(scan)
                        && self.accepts_candidate_role(candidate.role())
                        && !hints.rejects_candidate(self.analysis.view_db(), &candidate)?
                        && matcher.matches(candidate.symbol())?
                    {
                        symbols.push(candidate);
                    }
                }
            }
            return Ok(());
        }

        match self.query.search_scope() {
            ReferenceSearchScope::Crates(crates) => {
                for crate_ref in crates {
                    rg_std::check_cancel!(self.analysis, "reference crate scan");
                    let scan = ReferenceScanTarget {
                        crate_ref: *crate_ref,
                        file_id: None,
                    };
                    if visited.contains(&scan) {
                        continue;
                    }
                    visited.push(scan);
                    self.push_matching_scan_target_candidates(scan, matcher, hints, symbols)?;
                }
            }
            ReferenceSearchScope::Files(files) => {
                for file in files {
                    rg_std::check_cancel!(self.analysis, "reference file scan");
                    let scan = ReferenceScanTarget {
                        crate_ref: file.crate_ref,
                        file_id: Some(file.file_id),
                    };
                    if visited.contains(&scan) {
                        continue;
                    }
                    visited.push(scan);
                    self.push_matching_scan_target_candidates(scan, matcher, hints, symbols)?;
                }
            }
            ReferenceSearchScope::File { crate_ref, file_id } => {
                self.push_matching_scan_target_candidates(
                    ReferenceScanTarget {
                        crate_ref,
                        file_id: Some(file_id),
                    },
                    matcher,
                    hints,
                    symbols,
                )?;
            }
        }

        Ok(())
    }

    fn push_matching_scan_target_candidates(
        &self,
        scan: ReferenceScanTarget,
        matcher: &ReferenceDeclarationMatcher<'_, 'db>,
        hints: &ReferenceSearchHints,
        symbols: &mut Vec<SourceSymbol>,
    ) -> anyhow::Result<()> {
        let source_symbols = SourceSymbolIndex::new(self.analysis.view_db());
        let candidates =
            source_symbols.symbols_in_crate(scan.crate_ref, scan.file_id, &hints.selection)?;

        for candidate in candidates {
            rg_std::check_cancel!(self.analysis, "reference candidate matching");
            if !self.accepts_candidate_role(candidate.role()) {
                continue;
            }
            if hints.rejects_candidate(self.analysis.view_db(), &candidate)? {
                continue;
            }
            if matcher.matches(candidate.symbol())? {
                symbols.push(candidate);
            }
        }
        Ok(())
    }

    fn accepts_candidate_role(&self, role: SourceSymbolRole) -> bool {
        match role {
            SourceSymbolRole::Reference => true,
            SourceSymbolRole::Declaration => self.query.includes_declarations(),
            SourceSymbolRole::Structural => false,
        }
    }
}

/// Request-local declaration matcher used while scanning source occurrences.
struct ReferenceDeclarationMatcher<'a, 'db> {
    resolver: SourceSymbolResolver<'a, 'db>,
    declarations: &'a HashSet<DeclarationRef>,
}

impl<'a, 'db> ReferenceDeclarationMatcher<'a, 'db> {
    fn new(db: &'a IndexedViewDb<'db>, subject: &'a ReferenceSubject) -> Self {
        Self {
            resolver: SourceSymbolResolver::new(db),
            declarations: subject.declarations(),
        }
    }

    fn matches(&self, symbol: &SymbolAt) -> anyhow::Result<bool> {
        if let SymbolAt::Declaration { declaration, .. } = symbol
            && self.declarations.contains(declaration)
        {
            return Ok(true);
        }

        let candidate_declarations = self.resolver.declarations_for_symbol(symbol.clone())?;
        Ok(candidate_declarations
            .iter()
            .any(|candidate| self.declarations.contains(candidate)))
    }
}
