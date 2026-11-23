//! Resolve each selected item expansion inventory once during a rename.

use std::collections::{HashMap, hash_map::Entry};

use rg_def_map::{GeneratedSourceStoreId, MacroCopyCoverage, MacroCopyEvidence, MacroExpansionRef};
use rg_ir_model::{CrateRef, FileId, ItemStoreId, PackageSlot, Span};
use rg_item_tree::file_declaration_sources;
use rg_source::QueryContext;

use super::{DeclarationSourceResolver, ItemSourceView, SourceSelection};

pub struct ItemMacroCopyIndex<'db> {
    db: &'db QueryContext<'db>,
    stores: HashMap<(CrateRef, FileId, GeneratedSourceStoreId), Vec<IndexedMacroCall>>,
}

impl<'db> ItemMacroCopyIndex<'db> {
    pub fn new(db: &'db QueryContext<'db>) -> Self {
        Self {
            db,
            stores: HashMap::new(),
        }
    }

    pub fn record(
        &mut self,
        krate: CrateRef,
        file: FileId,
        edit: Span,
        evidence: &mut MacroCopyEvidence,
    ) -> ajika::Result<()> {
        let db = self.db;
        let members = DeclarationSourceResolver::new(db, krate)
            .member_stores(SourceSelection::all_at(file, edit.start))?;
        let stores = std::iter::once(GeneratedSourceStoreId::Target).chain(
            members.into_iter().filter_map(|store| match store.id {
                ItemStoreId::Impl(id) => Some(GeneratedSourceStoreId::ImplMembers(id)),
                ItemStoreId::Trait(id) => Some(GeneratedSourceStoreId::TraitMembers(id)),
                ItemStoreId::Scope => None,
            }),
        );
        for store in stores {
            let calls = match self.stores.entry((krate, file, store)) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => entry.insert(Self::index_store(db, krate, file, store)?),
            };
            // Several nested calls can share a fallback range. Use the largest end seen so far
            // to skip earlier calls without overlooking those enclosing ranges.
            let start = calls.partition_point(|call| call.max_end < edit.end);
            for call in calls[start..]
                .iter()
                .take_while(|call| call.span.start <= edit.start)
            {
                if !call.span.contains_span(edit) {
                    continue;
                }
                let Some(expansion) = &call.expansion else {
                    evidence.mark_unverified();
                    continue;
                };
                let copied = evidence.record(expansion.id, edit, &expansion.coverage);
                // A different semantic route may discard the argument altogether.
                if expansion.is_root && !copied {
                    evidence.mark_unverified();
                }
            }
        }
        Ok(())
    }

    fn index_store(
        db: &QueryContext<'_>,
        krate: CrateRef,
        file: FileId,
        store: GeneratedSourceStoreId,
    ) -> ajika::Result<Vec<IndexedMacroCall>> {
        let Some(inventory) =
            ItemSourceView::generated_sources(db, krate, store)?.macro_copies(db)?
        else {
            return Ok(Vec::new());
        };
        let mut calls = Vec::new();
        for &call in inventory.unverified_calls() {
            if call.anchor.file_id.raw_file_id() == file.as_raw()
                && let Some(span) =
                    file_declaration_sources(db, PackageSlot(call.anchor.package as usize), file)?
                        .token_span(call)
            {
                calls.push(IndexedMacroCall {
                    span,
                    max_end: span.end,
                    expansion: None,
                });
            }
        }
        for (source, copies) in inventory.expansions() {
            db.check_cancelled()?;
            let anchor = copies.call_site.anchor;
            if anchor.file_id.raw_file_id() != file.as_raw() {
                continue;
            }
            let inputs = file_declaration_sources(db, PackageSlot(anchor.package as usize), file)?;
            let Some(span) = inputs.token_span(copies.call_site) else {
                continue;
            };
            calls.push(IndexedMacroCall {
                span,
                max_end: span.end,
                expansion: Some(IndexedItemExpansion {
                    id: MacroExpansionRef::Item {
                        krate,
                        store,
                        source,
                    },
                    coverage: MacroCopyCoverage {
                        file,
                        copies: copies
                            .tokens
                            .iter()
                            .filter_map(|&(token, origin)| {
                                Some((token, inputs.token_span(origin)?))
                            })
                            .collect(),
                        transformed: copies
                            .transformed
                            .iter()
                            .filter_map(|&origin| inputs.token_span(origin))
                            .collect(),
                        forwarded: copies.forwarded.clone(),
                    },
                    is_root: copies.is_root,
                }),
            });
        }
        calls.sort_unstable_by_key(|call| call.span.start);
        let mut max_end = 0;
        for call in &mut calls {
            max_end = max_end.max(call.span.end);
            call.max_end = max_end;
        }
        Ok(calls)
    }
}

struct IndexedMacroCall {
    span: Span,
    max_end: u32,
    expansion: Option<IndexedItemExpansion>,
}

struct IndexedItemExpansion {
    id: MacroExpansionRef,
    coverage: MacroCopyCoverage,
    is_root: bool,
}
