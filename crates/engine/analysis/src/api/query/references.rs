//! Reference search over the facts already held by the analysis graph.
//!
//! Reference lookup intentionally scans known source facts instead of building a separate index.
//! The query owns the search surface and declaration-inclusion policy, while `ReferenceView`
//! projects declaration identities into source locations.

use rg_ir_model::{TargetRef, identity::DeclarationRef};
use rg_parse::FileId;

use crate::{
    api::{
        Analysis,
        source_symbol::SourceSymbolResolver,
        source_symbol::{SourceSymbol, SourceSymbolIndex, SourceSymbolRole},
        view::reference::ReferenceView,
    },
    model::{ReferenceLocation, SymbolAt},
};

/// Options for a source reference lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceQuery<'a> {
    search_scope: ReferenceSearchScope<'a>,
    declaration_policy: ReferenceDeclarationPolicy,
}

impl<'a> ReferenceQuery<'a> {
    /// Returns a query for explicit find-references requests.
    pub fn find_references(search_targets: &'a [TargetRef], include_declarations: bool) -> Self {
        let declaration_policy = if include_declarations {
            ReferenceDeclarationPolicy::IncludeUnscoped
        } else {
            ReferenceDeclarationPolicy::Exclude
        };

        Self {
            search_scope: ReferenceSearchScope::Targets(search_targets),
            declaration_policy,
        }
    }

    /// Returns a query scoped to one file inside one target.
    pub fn file_scoped(target: TargetRef, file_id: FileId) -> Self {
        Self {
            search_scope: ReferenceSearchScope::File { target, file_id },
            declaration_policy: ReferenceDeclarationPolicy::IncludeInSearchScope,
        }
    }

    /// Removes declaration locations from this query.
    pub fn without_declarations(mut self) -> Self {
        self.declaration_policy = ReferenceDeclarationPolicy::Exclude;
        self
    }

    fn search_scope(self) -> ReferenceSearchScope<'a> {
        self.search_scope
    }

    fn includes_declarations(self) -> bool {
        !matches!(self.declaration_policy, ReferenceDeclarationPolicy::Exclude)
    }

    fn accepts_declaration(self, target: TargetRef, file_id: FileId) -> bool {
        match self.declaration_policy {
            ReferenceDeclarationPolicy::Exclude => false,
            ReferenceDeclarationPolicy::IncludeUnscoped => true,
            ReferenceDeclarationPolicy::IncludeInSearchScope => match self.search_scope {
                ReferenceSearchScope::Targets(targets) => targets.contains(&target),
                ReferenceSearchScope::File {
                    target: selected_target,
                    file_id: selected_file_id,
                } => selected_target == target && selected_file_id == file_id,
            },
        }
    }
}

/// Source surface scanned for reference use-sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReferenceSearchScope<'a> {
    /// Scans all source candidates inside the listed targets.
    Targets(&'a [TargetRef]),
    /// Scans source candidates in one file inside one target.
    File { target: TargetRef, file_id: FileId },
}

/// How declaration locations should relate to the reference search surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReferenceDeclarationPolicy {
    /// Do not return declaration locations.
    Exclude,
    /// Return declarations even when they are outside `ReferenceSearchScope`.
    IncludeUnscoped,
    /// Return declarations only when they are inside `ReferenceSearchScope`.
    IncludeInSearchScope,
}

pub(crate) struct ReferenceResolver<'a, 'db, 'scope> {
    analysis: &'a Analysis<'db>,
    query: ReferenceQuery<'scope>,
}

impl<'a, 'db, 'scope> ReferenceResolver<'a, 'db, 'scope> {
    pub(crate) fn new(analysis: &'a Analysis<'db>, query: ReferenceQuery<'scope>) -> Self {
        Self { analysis, query }
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
        let Some(symbol) = self.analysis.symbol_at_for_query(target, file_id, offset)? else {
            return Ok(Vec::new());
        };
        let reference_view = ReferenceView::new(self.analysis);
        let declarations = self.unique_declarations_for_symbol(symbol)?;
        if declarations.is_empty() {
            return Ok(Vec::new());
        }

        let mut locations = Vec::new();
        if self.query.includes_declarations() {
            for location in reference_view.declaration_locations(&declarations)? {
                if self
                    .query
                    .accepts_declaration(location.target, location.file_id)
                {
                    locations.push(location);
                }
            }
        }

        for candidate in self.reference_candidates()? {
            if self.source_symbol_matches_declarations(candidate.symbol().clone(), &declarations)? {
                locations.push(ReferenceLocation {
                    target: candidate.target(),
                    file_id: candidate.file_id(),
                    span: candidate.span(),
                });
            }
        }

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

    fn unique_declarations_for_symbol(
        &self,
        symbol: SymbolAt,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let declarations =
            SourceSymbolResolver::new(self.analysis).declarations_for_symbol(symbol)?;
        let mut unique = Vec::new();
        for declaration in declarations {
            if !unique.contains(&declaration) {
                unique.push(declaration);
            }
        }
        Ok(unique)
    }

    fn source_symbol_matches_declarations(
        &self,
        symbol: SymbolAt,
        declarations: &[DeclarationRef],
    ) -> anyhow::Result<bool> {
        let candidate_declarations = self.unique_declarations_for_symbol(symbol)?;
        Ok(candidate_declarations
            .iter()
            .any(|candidate| declarations.contains(candidate)))
    }

    fn reference_candidates(&self) -> anyhow::Result<Vec<SourceSymbol>> {
        let mut candidates = Vec::new();
        let mut visited = Vec::new();

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
                    self.push_scan_target_candidates(scan, &mut candidates)?;
                }
            }
            ReferenceSearchScope::File { target, file_id } => {
                self.push_scan_target_candidates(
                    ReferenceScanTarget {
                        target,
                        file_id: Some(file_id),
                    },
                    &mut candidates,
                )?;
            }
        }

        Ok(candidates)
    }

    fn push_scan_target_candidates(
        &self,
        scan: ReferenceScanTarget,
        candidates: &mut Vec<SourceSymbol>,
    ) -> anyhow::Result<()> {
        for candidate in
            SourceSymbolIndex::new(self.analysis).symbols_in_target(scan.target, scan.file_id)?
        {
            match candidate.role() {
                SourceSymbolRole::Reference => candidates.push(candidate),
                SourceSymbolRole::Declaration if self.query.includes_declarations() => {
                    candidates.push(candidate);
                }
                SourceSymbolRole::Declaration | SourceSymbolRole::Structural => {}
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReferenceScanTarget {
    target: TargetRef,
    file_id: Option<FileId>,
}
