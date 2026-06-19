//! Reference searches over the facts already held by the analysis graph.
//!
//! Reference lookup intentionally scans known source facts instead of building a separate index.
//! The query owns the search surface, declaration-inclusion policy, and declaration projection.

use std::collections::HashSet;

use rg_ir_model::{TargetRef, identity::DeclarationRef};
use rg_ir_view::IndexedViewDb;
use rg_parse::FileId;
use rg_std::UniqueVec;

use crate::{
    Analysis,
    model::{ReferenceLocation, SymbolAt},
    source_symbol::{SourceSymbol, SourceSymbolIndex, SourceSymbolResolver, SourceSymbolRole},
};

mod search;
mod subject;

pub use search::{ReferenceQuery, ReferenceSearchFile, ReferenceSearchLabel};

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

    /// Returns source labels that are safe for request-local text prefiltering.
    pub(crate) fn reference_search_labels(
        analysis: &Analysis<'db>,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<ReferenceSearchLabel>> {
        let Some(symbol) = analysis.symbol_at_for_query(target, file_id, offset)? else {
            return Ok(Vec::new());
        };
        let declarations = Self::unique_declarations_for_analysis(analysis, symbol)?;
        let (_, hints) = ReferenceSubject::resolve(analysis.view_db(), &declarations)?;
        if hints.local_scan_target().is_some() {
            return Ok(Vec::new());
        }
        Ok(hints.exact_labels())
    }

    /// Finds references for the symbol under `offset` by scanning the requested use-site surface.
    ///
    /// Declaration locations are projected from the selected symbol before use-site scanning when
    /// requested, using the resolver's declaration scope policy.
    pub(crate) fn references(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<ReferenceLocation>> {
        let symbols = self.matching_source_symbols(target, file_id, offset)?;
        let mut locations = symbols
            .into_iter()
            .map(|symbol| ReferenceLocation {
                target: symbol.target(),
                file_id: symbol.file_id(),
                span: symbol.span(),
            })
            .collect::<Vec<_>>();

        locations.sort_by_key(|location| {
            (
                location.target.package.0,
                location.target.target.0,
                location.file_id.0,
                location.span.text.start,
                location.span.text.end,
            )
        });
        locations.dedup();
        Ok(locations)
    }

    pub(crate) fn matching_source_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        let Some(symbol) = self.analysis.symbol_at_for_query(target, file_id, offset)? else {
            return Ok(Vec::new());
        };
        let declarations = self.unique_declarations_for_symbol(symbol)?;
        if declarations.is_empty() {
            return Ok(Vec::new());
        }

        self.source_symbols_matching_declarations(&declarations)
    }

    pub(crate) fn source_symbols_matching_declarations(
        &self,
        declarations: &[DeclarationRef],
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        if declarations.is_empty() {
            return Ok(Vec::new());
        }

        let (subject, hints) = ReferenceSubject::resolve(self.analysis.view_db(), declarations)?;
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
                if !self
                    .query
                    .accepts_declaration(location.target, location.file_id)
                {
                    continue;
                }
                if symbols.iter().any(|symbol| {
                    symbol.role() == SourceSymbolRole::Declaration
                        && symbol.target() == location.target
                        && symbol.file_id() == location.file_id
                        && symbol.span() == location.span
                }) {
                    continue;
                }
                symbols.push(SourceSymbol::plain_declaration(
                    location.declaration,
                    location.target,
                    location.file_id,
                    location.span,
                ));
            }
        }

        symbols.sort_by_key(|symbol| {
            (
                symbol.target().package.0,
                symbol.target().target.0,
                symbol.file_id().0,
                symbol.span().text.start,
                symbol.span().text.end,
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

        if let Some(scan) = hints.local_scan_target() {
            if self.query.accepts_scan_target(scan) {
                self.push_matching_scan_target_candidates(scan, matcher, hints, symbols)?;
            }
            return Ok(());
        }

        match self.query.search_scope() {
            ReferenceSearchScope::Targets(targets) => {
                for target in targets {
                    let scan = ReferenceScanTarget {
                        target: *target,
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
                    let scan = ReferenceScanTarget {
                        target: file.target,
                        file_id: Some(file.file_id),
                    };
                    if visited.contains(&scan) {
                        continue;
                    }
                    visited.push(scan);
                    self.push_matching_scan_target_candidates(scan, matcher, hints, symbols)?;
                }
            }
            ReferenceSearchScope::File { target, file_id } => {
                self.push_matching_scan_target_candidates(
                    ReferenceScanTarget {
                        target,
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
        for candidate in SourceSymbolIndex::new(self.analysis.view_db())
            .symbols_in_target(scan.target, scan.file_id)?
        {
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
    declarations: HashSet<DeclarationRef>,
}

impl<'a, 'db> ReferenceDeclarationMatcher<'a, 'db> {
    fn new(db: &'a IndexedViewDb<'db>, subject: &ReferenceSubject) -> Self {
        Self {
            resolver: SourceSymbolResolver::new(db),
            declarations: subject.declarations().clone(),
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
