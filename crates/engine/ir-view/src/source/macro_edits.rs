//! Collect every interpretation of a written macro argument before allowing an edit.
//!
//! Editing `name` in `make!(name)` affects every copy of that input, including copies passed to
//! another macro or used as tokens by `stringify!`. Use the copy inventories to find the outputs
//! affected by the edit, and the occurrence scans to find the names analysis can resolve.

use std::collections::{HashMap, HashSet, hash_map::Entry};

use rg_def_map::{ItemSourceKind, MacroCopyCoverage, MacroCopyRef, MacroExpansionRef};
use rg_ir_model::{BodyKey, CrateRef, FileId, Span};
use rg_semantic_ir::{ItemMacroCopyIndex, ItemMacroCoverage, SourceSelection};
use rg_std::UniqueVec;

use super::{IndexedSourceOccurrence, SourceOccurrenceView};
use crate::IndexedViewDb;

type BodyOccurrences = HashMap<(FileId, Span), Vec<IndexedSourceOccurrence>>;

/// Read the sources for a rename in one snapshot. A rename can check several spans in the
/// same body, so keep its occurrence scan and look up each span in that scan.
pub struct MacroEditSourceView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
    items: ItemMacroCopyIndex<'db>,
    bodies: HashMap<BodyKey, BodyOccurrences>,
}

impl<'a, 'db> MacroEditSourceView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self {
            db,
            items: db.item_macro_copy_index(),
            bodies: HashMap::new(),
        }
    }

    /// Return the occurrences at the edit and add this target's copy coverage to the evidence.
    /// The caller can reuse the evidence across targets that read the same written file.
    pub fn occurrences(
        &mut self,
        krate: CrateRef,
        file: FileId,
        span: Span,
        evidence: &mut MacroCopyEvidence,
    ) -> anyhow::Result<Vec<IndexedSourceOccurrence>> {
        self.items
            .for_each_at(krate, file, span, |item| match item {
                ItemMacroCoverage::Expanded {
                    id,
                    coverage,
                    is_root,
                } => {
                    let copied = evidence.record(id, span, coverage);
                    // A different semantic route may discard the argument altogether.
                    if is_root && !copied {
                        evidence.mark_unverified();
                    }
                }
                ItemMacroCoverage::Unverified => evidence.mark_unverified(),
            })?;
        let view = SourceOccurrenceView::new(self.db);
        let selection = SourceSelection::all_at(file, span.start);
        let mut occurrences = view.declaration_occurrences(krate, selection)?;
        occurrences.retain(|entry| entry.written_span() == (file, span));
        let mut keys = UniqueVec::new();
        for owner in self.db.body_ir.owners(krate, selection)? {
            rg_std::check_cancel!(self.db, "macro argument owners");
            // Narrow the candidate bodies by their declaration ranges before opening their source
            // or inferred facts.
            if !self
                .db
                .body_ir
                .declaration_location(krate, owner.source, owner.location)?
                .span
                .contains_span(span)
            {
                continue;
            }
            let key = BodyKey {
                krate,
                owner: owner.owner,
            };
            if !keys.push(key) {
                continue;
            }
            for (expansion, coverage) in self.db.body_ir.macro_copy_coverage(key)? {
                if coverage.file == file {
                    evidence.record(expansion, span, coverage);
                }
            }
            let indexed = match self.bodies.entry(key) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let mut indexed = BodyOccurrences::new();
                    for occurrence in view.occurrences_in_body(key)? {
                        indexed
                            .entry(occurrence.written_span())
                            .or_default()
                            .push(occurrence);
                    }
                    entry.insert(indexed)
                }
            };
            // The same written function can be included with a different macro in scope.
            // An opaque call in that route may provide no expansion or indexed name at all.
            if let Some(at_edit) = indexed.get(&(file, span)) {
                occurrences.extend(at_edit.iter().cloned());
            } else if matches!(owner.source.kind, ItemSourceKind::ItemTree(_)) {
                evidence.mark_unverified();
            }
        }
        Ok(occurrences)
    }
}

/// The proof obligations for one proposed edit, found across the selected source routes.
#[derive(Default)]
pub struct MacroCopyEvidence {
    copies: HashSet<MacroCopyRef>,
    forwarded: HashSet<MacroCopyRef>,
    blocked: bool,
}

impl MacroCopyEvidence {
    fn mark_unverified(&mut self) {
        self.blocked = true;
    }

    /// Check that the selected copy was verified and every verified copy belongs to the inventory.
    /// If an expansion forwards a copy to a nested call, the nested outputs carry the obligation.
    /// Each copy at the end of that chain needs its own verified occurrence. An opaque call or a
    /// transformed input can still block the edit.
    pub fn is_complete(&self, selected: MacroCopyRef, verified: &HashSet<MacroCopyRef>) -> bool {
        !self.blocked
            && verified.contains(&selected)
            && verified.is_subset(&self.copies)
            && self
                .copies
                .difference(&self.forwarded)
                .all(|copy| verified.contains(copy))
    }

    fn record(
        &mut self,
        expansion: MacroExpansionRef,
        edit: Span,
        coverage: &MacroCopyCoverage,
    ) -> bool {
        // A format capture edits an inner range of the literal. Every copy of the literal
        // still needs to account for that exact inner edit.
        let mut found = false;
        for &(token, input) in &coverage.copies {
            if input.contains_span(edit) {
                found = true;
                self.copies.insert(MacroCopyRef { expansion, token });
            }
        }
        self.blocked |= coverage
            .transformed
            .iter()
            .any(|input| input.contains_span(edit));
        self.forwarded.extend(coverage.forwarded.iter().copied());
        found
    }
}
