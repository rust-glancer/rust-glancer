//! Reference search over the facts already held by the analysis graph.
//!
//! The initial references implementation intentionally scans known source facts instead of building
//! a separate index. That keeps the feature aligned with goto/hover resolution: every candidate is
//! normalized through the same declaration resolver before we compare declaration identities.

use rg_def_map::TargetRef;
use rg_parse::FileId;

use crate::{
    api::{
        Analysis,
        resolve::declaration::SymbolDeclarationResolver,
        source_symbol::{SourceSymbol, SourceSymbolIndex, SourceSymbolRole},
        view::declaration::{DeclarationRef, DeclarationView},
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
        let subjects = self.subjects_for_symbol(symbol.clone())?;
        if subjects.is_empty() {
            return Ok(Vec::new());
        }

        let mut locations = Vec::new();
        if self.query.includes_declarations() {
            self.push_selected_declarations(symbol, &mut locations)?;
        }

        for candidate in self.reference_candidates()? {
            let candidate_subjects = self.subjects_for_symbol(candidate.symbol().clone())?;
            if candidate_subjects
                .iter()
                .any(|candidate| subjects.contains(candidate))
            {
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

    fn push_selected_declarations(
        &self,
        symbol: SymbolAt,
        locations: &mut Vec<ReferenceLocation>,
    ) -> anyhow::Result<()> {
        for declaration_ref in self.subjects_for_symbol(symbol)? {
            let Some(declaration) =
                DeclarationView::new(self.analysis).declaration(declaration_ref)?
            else {
                continue;
            };
            if !self
                .query
                .accepts_declaration(declaration.target(), declaration.file_id())
            {
                continue;
            }
            locations.push(ReferenceLocation {
                target: declaration.target(),
                file_id: declaration.file_id(),
                span: declaration.selection_span(),
            });
        }
        Ok(())
    }

    fn subjects_for_symbol(&self, symbol: SymbolAt) -> anyhow::Result<Vec<DeclarationRef>> {
        let declarations =
            SymbolDeclarationResolver::new(self.analysis).declarations_for_symbol(symbol)?;
        let mut subjects = Vec::new();
        for declaration in declarations {
            if !subjects.contains(&declaration) {
                subjects.push(declaration);
            }
        }
        Ok(subjects)
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
