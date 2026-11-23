//! Find item macro invocations covering a written range.
//!
//! Index invocation ranges in the active snapshot, once per target, file, and source store.
//! Open the expansion's copy coverage when its invocation contains the edit. Keep separate
//! entries for each source route even when the written ranges match. Two `include!` calls can
//! put the same source under different macros in scope, and rename must check both results.

use std::collections::{HashMap, hash_map::Entry};

use rg_def_map::{
    GeneratedSourceStoreId, ItemMacroCopySource, MacroCopyCoverage, MacroExpansionRef,
};
use rg_ir_model::{CrateRef, FileId, ItemStoreId, PackageSlot, Span};
use rg_item_tree::file_declaration_sources;
use rg_source::QueryContext;

use super::{DeclarationSourceResolver, ItemSourceView, SourceSelection};

/// What the item expansion recorded for an invocation covering the range.
pub enum ItemMacroCoverage<'db> {
    Expanded {
        id: MacroExpansionRef,
        coverage: &'db MacroCopyCoverage,
        /// This call has its own written invocation. A nested generated call can share the
        /// enclosing invocation's range as a fallback.
        is_root: bool,
    },
    /// The invocation has a known source range and no usable copy inventory.
    Unverified,
}

pub struct ItemMacroCopyIndex<'db> {
    db: &'db QueryContext<'db>,
    stores: HashMap<(CrateRef, FileId, GeneratedSourceStoreId), Vec<IndexedMacroCall<'db>>>,
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
        mut visit: impl FnMut(ItemMacroCoverage<'db>),
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
                    let coverage = match call.expansion {
                        Some((id, source)) => ItemMacroCoverage::Expanded {
                            id,
                            coverage: source.coverage(db)?,
                            is_root: source.is_root,
                        },
                        None => ItemMacroCoverage::Unverified,
                    };
                    visit(coverage);
                }
            }
        }
        Ok(())
    }

    fn index_store(
        db: &'db QueryContext<'db>,
        krate: CrateRef,
        file: FileId,
        store: GeneratedSourceStoreId,
    ) -> ajika::Result<Vec<IndexedMacroCall<'db>>> {
        let Some(sources) =
            ItemSourceView::generated_sources(db, krate, store)?.macro_copy_sources(db)?
        else {
            return Ok(Vec::new());
        };
        let mut calls = Vec::new();
        for &call in sources.unverified_calls() {
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
        for (source, copy_source) in sources.expansions() {
            db.check_cancelled()?;
            let anchor = copy_source.call_site.anchor;
            if anchor.file_id.raw_file_id() != file.as_raw() {
                continue;
            }
            let inputs = file_declaration_sources(db, PackageSlot(anchor.package as usize), file)?;
            let Some(span) = inputs.token_span(copy_source.call_site) else {
                continue;
            };
            calls.push(IndexedMacroCall {
                span,
                max_end: span.end,
                expansion: Some((
                    MacroExpansionRef::Item {
                        krate,
                        store,
                        source,
                    },
                    copy_source,
                )),
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

struct IndexedMacroCall<'db> {
    span: Span,
    max_end: u32,
    expansion: Option<(MacroExpansionRef, &'db ItemMacroCopySource)>,
}
