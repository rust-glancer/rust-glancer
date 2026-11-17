//! Build-time body-aware routing for shared DefMap and item-store queries.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use rg_arena::Arena;
use rg_def_map::{DefMap, DefMapSource, DefinitionReader};
use rg_ir_model::{
    BodyId, BodyRef, CrateRef, DefMapRef, ItemStoreRef, LocalDefRef, ModuleRef, SemanticItemRef,
};
use rg_semantic_ir::{
    AssociatedItemStore, ItemLookupQuery, ItemReader, SemanticItemSource, SemanticItemView,
};
use rg_source::ReadError;
use rg_std::{CancellationToken, Shrink as _, UniqueVec};

use crate::{BodyFactsStore, BodyLocalItems, PreparedBodyStore, resolution::InferenceContext};

/// Provides saved crate facts plus the local declarations of one outer owner and its nested bodies.
///
/// The frozen query path can already route arbitrary body origins through `BodyReader`.
/// Preparation and inference need the same routing before those bodies have been published, so
/// this source borrows their local stores directly.
#[derive(Clone, Copy)]
pub(super) struct BodyBuildQuerySource<'a, 'db> {
    def_map: &'a DefinitionReader<'db>,
    semantic_ir: &'a ItemReader<'db>,
    crate_ref: CrateRef,
    body_slots: &'a HashMap<BodyRef, BodyId>,
    body_local_items: &'a Arena<BodyId, BodyLocalItems>,
}

impl<'a, 'db> BodyBuildQuerySource<'a, 'db> {
    pub(super) fn new(
        def_map: &'a DefinitionReader<'db>,
        semantic_ir: &'a ItemReader<'db>,
        crate_ref: CrateRef,
        body_slots: &'a HashMap<BodyRef, BodyId>,
        body_local_items: &'a Arena<BodyId, BodyLocalItems>,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            crate_ref,
            body_slots,
            body_local_items,
        }
    }

    fn body_local_items(self, body_ref: BodyRef) -> Option<&'a BodyLocalItems> {
        if body_ref.crate_ref != self.crate_ref {
            return None;
        }

        let slot = *self.body_slots.get(&body_ref)?;
        self.body_local_items.get(slot)
    }
}

impl DefMapSource for BodyBuildQuerySource<'_, '_> {
    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, ReadError> {
        match origin {
            DefMapRef::Crate(crate_ref) => self.def_map.def_map(crate_ref),
            DefMapRef::Body(body_ref) => Ok((*self)
                .body_local_items(body_ref)
                .map(BodyLocalItems::def_map)),
        }
    }

    fn crate_is_proc_macro(&self, crate_ref: CrateRef) -> Result<bool, ReadError> {
        self.def_map.crate_is_proc_macro(crate_ref)
    }

    fn target_pointer_width(&self, crate_ref: CrateRef) -> Result<Option<u32>, ReadError> {
        self.def_map.target_pointer_width(crate_ref)
    }

    fn extern_root(&self, crate_ref: CrateRef, name: &str) -> Result<Option<ModuleRef>, ReadError> {
        self.def_map.extern_root(crate_ref, name)
    }

    fn extern_roots(&self, crate_ref: CrateRef) -> Result<Vec<(String, ModuleRef)>, ReadError> {
        self.def_map.extern_roots(crate_ref)
    }

    fn prelude_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, ReadError> {
        self.def_map.prelude_module(crate_ref)
    }

    fn item_lookup_dependencies(
        &self,
        crate_ref: CrateRef,
    ) -> Result<UniqueVec<CrateRef>, ReadError> {
        self.def_map.item_lookup_dependencies(crate_ref)
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, ReadError> {
        self.def_map.root_module(crate_ref)
    }
}

impl<'source, 'db> SemanticItemSource<'source> for &'source BodyBuildQuerySource<'_, 'db> {
    fn associated_items(
        &self,
        store: ItemStoreRef,
    ) -> Result<Option<&'source AssociatedItemStore>, ReadError> {
        match store.origin {
            DefMapRef::Crate(_) => self.semantic_ir.associated_items(store),
            DefMapRef::Body(body) => Ok(self
                .body_local_items(body)
                .map(BodyLocalItems::item_store)
                .and_then(|items| items.associated_items(store))),
        }
    }

    fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'source>>, ReadError> {
        match item.origin() {
            DefMapRef::Crate(_) => self.semantic_ir.item(item),
            DefMapRef::Body(body) => Ok(self
                .body_local_items(body)
                .map(BodyLocalItems::item_store)
                .and_then(|items| items.semantic_item_view(item))),
        }
    }

    fn item_for_local_def(
        &self,
        definition: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, ReadError> {
        match definition.origin {
            DefMapRef::Crate(_) => self.semantic_ir.item_for_local_def(definition),
            DefMapRef::Body(body) => Ok(self
                .body_local_items(body)
                .map(BodyLocalItems::item_store)
                .and_then(|items| {
                    items
                        .item_for_local_def(definition.local_def)
                        .map(|item| item.semantic_ref(definition.origin))
                })),
        }
    }

    fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'source>>, ReadError> {
        match origin {
            DefMapRef::Crate(_) => self.semantic_ir.all_items(origin),
            DefMapRef::Body(body) => Ok(self
                .body_local_items(body)
                .map(BodyLocalItems::item_store)
                .map(|items| items.semantic_items().collect())
                .unwrap_or_default()),
        }
    }
}

impl PreparedBodyStore {
    /// Saved and edited owners use the same inference kernel and the same lexical overlay.
    /// This method cannot see source maps: every dependency it observes contributes to meaning.
    pub(crate) fn infer(
        &self,
        definitions: &DefinitionReader<'_>,
        items: &ItemReader<'_>,
        lookup: &ItemLookupQuery<'_>,
        cancellation: &CancellationToken,
    ) -> anyhow::Result<BodyFactsStore> {
        let body_slots = self
            .body_refs
            .iter_with_ids()
            .map(|(slot, body)| (*body, slot))
            .collect();
        let mut facts = Arena::with_capacity(self.bodies.len());
        let Some(first) = self.body_refs.as_slice().first() else {
            return Ok(BodyFactsStore { bodies: facts });
        };
        let source = BodyBuildQuerySource::new(
            definitions,
            items,
            first.crate_ref,
            &body_slots,
            &self.local_items,
        );
        for (slot, body) in self.bodies.iter_with_ids() {
            rg_std::check_cancel!(cancellation, "body inference");
            let started = Instant::now();
            let result = InferenceContext::new(
                &source,
                &source,
                lookup,
                self.body_refs[slot],
                body,
                cancellation,
            )
            .infer_body()?;
            let elapsed = started.elapsed();
            if elapsed >= Duration::from_secs(1) {
                // Keep slow-body diagnostics independent of source queries. A position read here
                // would make location changes invalidate otherwise reusable inference.
                tracing::debug!(
                    body = ?self.body_refs[slot],
                    owner = ?body.owner(),
                    elapsed_ms = elapsed.as_millis(),
                    expression_count = body.exprs().len(),
                    binding_count = body.bindings().len(),
                    statement_count = body.statements().len(),
                    "slow body resolution"
                );
            }
            let allocated = facts.alloc(result);
            debug_assert_eq!(allocated, slot);
        }
        let mut facts = BodyFactsStore { bodies: facts };
        facts.shrink_to_fit();
        Ok(facts)
    }
}
