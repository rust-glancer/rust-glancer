//! Collect declarations and append the nested bodies discovered in them.
//!
//! Every worklist entry receives its local DefMap and item store before semantic resolution starts.

use anyhow::Context as _;
use rg_cfg_eval::CfgEvaluator;
use rg_def_map::DefinitionReader;
use rg_ir_model::{BodyId, BodyRef, DefMapRef, ItemOwner, ModuleRef, StaticRef};
use rg_item_tree::SourceLocations as _;
use rg_semantic_ir::{ItemReader, ItemStore};
use rg_std::CancellationToken;

use super::BodyStoreBuilder;
use crate::{
    BodyData, BodyLocalItems, BodyOwner, BodySourceStore,
    build::{
        local_items::LocalItemSource,
        lower::{BodyLoweringTask, BodyMacroExpansion, BodyTaskLowering},
        query_source::BodyBuildQuerySource,
    },
};

impl BodyStoreBuilder<'_> {
    // Walk every known body, collecting local facts and lowering newly discovered nested bodies.
    // This is a worklist rather than recursive descent: collecting one body can append nested
    // fn/const/static bodies, and the loop visits those appended bodies before resolution starts.
    pub(crate) fn materialize_body_local_items(
        &mut self,
        def_map: &DefinitionReader<'_>,
        semantic_ir: &ItemReader<'_>,
        cfg: CfgEvaluator<'_>,
    ) -> anyhow::Result<()> {
        self.body_local_items.clear();
        // `body_local_items` is the cursor into `lowered_bodies`: each collected slot means that
        // body has its local DefMap/item store ready. Nested lowering may extend `lowered_bodies`,
        // so the loop stops only once every appended body has been collected too.
        let mut macro_expansion =
            BodyMacroExpansion::new(self.edition, def_map, cfg, self.cancellation.clone());
        while self.body_local_items.len() < self.lowered_bodies.bodies().len() {
            self.db.check_cancelled()?;
            let body = self.body_local_items.next_id();
            let body_ref = self.body_ref(body);
            let items = self
                .collect_body_local_items(body, def_map, semantic_ir)
                .context("collect body-local declarations")?;
            let body_data = self.lowered_bodies.bodies()[body].body();
            let nested_tasks = Self::nested_body_tasks(
                body_ref,
                body_data,
                &self.lowered_bodies.bodies()[body].sources,
                &items,
                &self.cancellation,
            )
            .context("collect nested body tasks")?;
            let allocated = self.body_local_items.alloc(items);
            debug_assert_eq!(allocated, body);

            if !nested_tasks.is_empty() {
                BodyTaskLowering::new(
                    self.db,
                    self.edition,
                    &mut self.lowered_bodies,
                    cfg,
                    &self.cancellation,
                )
                .lower_tasks(&nested_tasks, &mut macro_expansion)?;
            }
        }

        Ok(())
    }

    // Collects the local items within a single already-lowered body.
    #[rg_std::cancelable("body declaration worklist", token = self.cancellation)]
    fn collect_body_local_items(
        &self,
        body: BodyId,
        def_map: &DefinitionReader<'_>,
        semantic_ir: &ItemReader<'_>,
    ) -> anyhow::Result<BodyLocalItems> {
        let body_ref = self.body_ref(body);
        let lowered = &self.lowered_bodies.bodies()[body];
        let body = lowered.body();

        // Finalization can see previously collected body-local DefMaps. This is what lets nested
        // bodies import names from the body scope that declared them.
        let source =
            BodyBuildQuerySource::new(def_map, semantic_ir, self.key, &self.body_local_items);
        LocalItemSource::for_body(body, &lowered.sources)
            .collect(body_ref, source, &self.cancellation)
            .context("collect body-local declarations")
    }

    fn nested_body_tasks(
        body_ref: BodyRef,
        body: &BodyData,
        sources: &BodySourceStore,
        items: &BodyLocalItems,
        cancellation: &CancellationToken,
    ) -> anyhow::Result<Vec<BodyLoweringTask>> {
        let def_map = items.def_map();
        let item_store = items.item_store();
        let body_owner = body.owner();
        let fallback_module = body.fallback_module();
        let origin = DefMapRef::Body(body_ref);
        let mut tasks = Vec::new();

        // Visit scope declarations and each owner arena. Member identity includes the trait or
        // impl, while type lookup still starts from its lexical module.
        for (function_ref, function_data) in items.functions_with_refs() {
            rg_std::check_cancel!(cancellation, "nested body task");
            if function_ref.store.origin != origin {
                continue;
            }
            if body_owner == BodyOwner::Function(function_ref) {
                continue;
            }
            let span = sources
                .location(function_data.location)
                .context("body declaration occurrence is absent")?
                .span;
            // Required trait methods and foreign functions live in the item store but do not own
            // a body that can become a nested lowering task.
            if !function_data.signature.has_body() {
                continue;
            }
            let Some(owner_module) =
                Self::owner_module_for_body_item_owner(item_store, function_data.owner)
            else {
                continue;
            };
            tasks.push(BodyLoweringTask {
                owner: BodyOwner::Function(function_ref),
                owner_module,
                fallback_module,
                file_id: function_data.source.file_id,
                span,
            });
        }

        for (const_ref, const_data) in items.consts_with_refs() {
            rg_std::check_cancel!(cancellation, "nested body task");
            if body_owner == BodyOwner::Const(const_ref) {
                continue;
            }
            let span = sources
                .location(const_data.location)
                .context("body declaration occurrence is absent")?
                .span;
            let Some(owner_module) =
                Self::owner_module_for_body_item_owner(item_store, const_data.owner)
            else {
                continue;
            };
            tasks.push(BodyLoweringTask {
                owner: BodyOwner::Const(const_ref),
                owner_module,
                fallback_module,
                file_id: const_data.source.file_id,
                span,
            });
        }

        // Foreign statics have no initializer to lower. Unlike functions, their declaration data
        // has no `has_body` bit, so the retained extern-block owner carries that distinction.
        for (static_id, static_data) in item_store.statics().iter_with_ids() {
            rg_std::check_cancel!(cancellation, "nested body task");
            let static_ref = StaticRef::new(origin, static_id);
            if body_owner == BodyOwner::Static(static_ref) {
                continue;
            }
            let span = sources
                .location(static_data.location)
                .context("body declaration occurrence is absent")?
                .span;
            if def_map
                .foreign_block(static_data.local_def.local_def)
                .is_some()
            {
                continue;
            }
            tasks.push(BodyLoweringTask {
                owner: BodyOwner::Static(static_ref),
                owner_module: static_data.owner,
                fallback_module,
                file_id: static_data.source.file_id,
                span,
            });
        }

        tasks.sort_by_key(|task| {
            let span = task.span;
            (task.file_id, span.start, span.end)
        });
        Ok(tasks)
    }

    fn owner_module_for_body_item_owner(
        item_store: &ItemStore,
        owner: ItemOwner,
    ) -> Option<ModuleRef> {
        match owner {
            ItemOwner::Module(module) => Some(module),
            ItemOwner::Trait(trait_id) => item_store.trait_data(trait_id).map(|data| data.owner),
            ItemOwner::Impl(impl_id) => item_store.impl_data(impl_id).map(|data| data.owner),
        }
    }
}
