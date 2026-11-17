//! Finds the semantic trait implementation whose item list owns a cursor.
//!
//! Trait implementations can be crate-level items or body-local items. Their source spans live
//! beside different semantic item stores, so this scanner checks both and exposes one source-site
//! shape to completion.

use rg_def_map::DefMapSource as _;
use rg_ir_model::{CrateRef, DefMapRef, FileId, ImplRef, SemanticItemRef, TraitDefRef};
use rg_semantic_ir::{ItemStoreQuery, SemanticItemView};
use rg_source::ReadError;

use super::NarrowestSourceSite;
use crate::IndexedViewDb;

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
    pub(crate) fn site(&self) -> Result<Option<TraitImplSourceSite>, ReadError> {
        let mut best = NarrowestSourceSite::new();
        let items = ItemStoreQuery::new(self.db);
        self.scan_items(
            items.semantic_items_for_origin(DefMapRef::Crate(self.crate_ref))?,
            &mut best,
        )?;

        // Rust permits item declarations inside bodies, including trait implementations. Owner spans
        // let us select only bodies that can contain this cursor.
        for (body_ref, _) in
            self.db
                .body_ir
                .bodies_at(self.crate_ref, self.file_id, self.offset, None)?
        {
            self.scan_items(
                items.semantic_items_for_origin(DefMapRef::Body(body_ref))?,
                &mut best,
            )?;
        }
        Ok(best.finish())
    }

    fn scan_items(
        &self,
        items: Vec<SemanticItemView<'_>>,
        best: &mut NarrowestSourceSite<TraitImplSourceSite>,
    ) -> Result<(), ReadError> {
        for item in items {
            let SemanticItemRef::Impl(impl_ref) = item.item() else {
                continue;
            };
            let Some(data) = item.impl_data() else {
                continue;
            };
            let Some(trait_ref) = data.resolved_trait_ref.as_option().copied() else {
                continue;
            };
            let Some(source) = self.db.local_impl_data(data.local_impl)? else {
                continue;
            };
            if source.file_id != self.file_id {
                continue;
            }
            let span = self
                .db
                .source_span(impl_ref.origin, source.source, source.location)?;
            if !span.touches(self.offset) {
                continue;
            }
            best.consider(
                TraitImplSourceSite {
                    impl_ref,
                    trait_ref,
                },
                span.len(),
            );
        }
        Ok(())
    }
}
