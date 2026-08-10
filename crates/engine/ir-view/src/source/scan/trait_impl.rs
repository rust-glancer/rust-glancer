//! Finds the semantic trait implementation whose item list owns a cursor.
//!
//! Trait implementations can be crate-level items or body-local items. Their source spans live
//! beside different semantic item stores, so this scanner checks both and exposes one source-site
//! shape to completion.

use rg_def_map::DefMapSource as _;
use rg_ir_model::{CrateRef, ImplRef, TraitDefRef};
use rg_package_store::PackageStoreError;
use rg_parse::FileId;

use crate::IndexedViewDb;

use super::NarrowestSourceSite;

/// Resolved trait implementation selected by its enclosing source span.
///
/// Inherent impls and trait impls with an unresolved header are skipped because they cannot supply
/// a trait declaration whose missing members can be projected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TraitImplSourceSite {
    pub(crate) impl_ref: ImplRef,
    pub(crate) trait_ref: TraitDefRef,
}

/// Selects the narrowest trait impl containing one source offset.
///
/// Nested body-local items can overlap their containing declaration, so source length is used to
/// keep the innermost matching impl.
pub(crate) struct TraitImplSourceSiteScanner<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
    crate_ref: CrateRef,
    file_id: FileId,
    offset: u32,
}

impl<'a, 'db> TraitImplSourceSiteScanner<'a, 'db> {
    pub(crate) fn new(
        db: &'a IndexedViewDb<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> Self {
        Self {
            db,
            crate_ref,
            file_id,
            offset,
        }
    }

    /// Search both crate-level and body-local semantic item stores.
    pub(crate) fn site(&self) -> Result<Option<TraitImplSourceSite>, PackageStoreError> {
        let mut best = NarrowestSourceSite::new();
        if let Some(items) = self.db.semantic_ir.items(self.crate_ref)? {
            self.scan_store(items, &mut best)?;
        }

        // Rust permits item declarations inside bodies, including trait implementations. Loading
        // only bodies from this file preserves the store's normal file-granular residency.
        for (body_ref, _) in self.db.body_ir.bodies(self.crate_ref, Some(self.file_id))? {
            if let Some(items) = self.db.body_ir.body_item_store(body_ref)? {
                self.scan_store(items, &mut best)?;
            }
        }
        Ok(best.finish())
    }

    fn scan_store(
        &self,
        items: &rg_semantic_ir::ItemStore,
        best: &mut NarrowestSourceSite<TraitImplSourceSite>,
    ) -> Result<(), PackageStoreError> {
        for (impl_ref, data) in items.impls_with_refs() {
            let Some(trait_ref) = data.resolved_trait_ref.as_option().copied() else {
                continue;
            };
            let Some(source) = self.db.local_impl_data(data.local_impl)? else {
                continue;
            };
            if source.file_id != self.file_id || !source.span.touches(self.offset) {
                continue;
            }
            best.consider(
                TraitImplSourceSite {
                    impl_ref,
                    trait_ref,
                },
                source.span.len(),
            );
        }
        Ok(())
    }
}
