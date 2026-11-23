//! Find item macro invocations covering a written range.
//!
//! DefMap records the input token origins for each expansion. This index resolves those origins
//! into spans in the active snapshot, once per target, file, and source store. A file reached
//! through multiple `include!` calls can have several owners with their own expansion results.

use std::collections::{HashMap, hash_map::Entry};

use rg_def_map::{GeneratedSourceStoreId, MacroCopyCoverage, MacroExpansionRef};
use rg_ir_model::{CrateRef, FileId, ItemStoreId, PackageSlot, Span};
use rg_item_tree::file_declaration_sources;
use rg_source::QueryContext;

use super::{DeclarationSourceResolver, ItemSourceView, SourceSelection};

/// What the item expansion recorded for an invocation covering the range.
pub enum ItemMacroCoverage {
    Expanded {
        id: MacroExpansionRef,
        coverage: MacroCopyCoverage,
        /// This call has its own written invocation. A nested generated call can share the
        /// enclosing invocation's range as a fallback.
        is_root: bool,
    },
    /// The invocation has a known source range and no usable copy inventory.
    Unverified,
}

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

    /// Visit each invocation that contains the whole edit, including overlapping invocations
    /// from different source owners.
    pub fn for_each_at(
        &mut self,
        krate: CrateRef,
        file: FileId,
        edit: Span,
        mut visit: impl FnMut(&ItemMacroCoverage),
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
                if call.span.contains_span(edit) {
                    visit(&call.coverage);
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
                    coverage: ItemMacroCoverage::Unverified,
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
                coverage: ItemMacroCoverage::Expanded {
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
                },
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
    coverage: ItemMacroCoverage,
}
