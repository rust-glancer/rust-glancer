//! Collect declarations and append the nested bodies discovered in them.
//!
//! Every worklist entry receives its local DefMap and item store before semantic resolution starts.
//! Current roots also recover their owner identity from the declarations collected here.

use anyhow::Context as _;
use rg_cfg_eval::CfgEvaluator;
use rg_def_map::{DefMap, DefMapReadTxn};
use rg_ir_model::{
    BodyId, BodyRef, BodySource, ConstRef, DefMapRef, FileId, ItemOwner, ModuleRef, Span, StaticRef,
};
use rg_semantic_ir::{ItemStore, SemanticIrReadTxn};

use crate::{
    BodyLocalItems, BodyOwner,
    build::{
        local_items::LocalItemSource,
        lower::{
            BodyLoweringTask, BodyMacroExpansion, BodyTaskLowering, BodyTaskSource,
            CurrentRootItems, LoweredBodyTask,
        },
        query_source::BodyBuildQuerySource,
    },
};

use super::CrateBodyBuildState;

impl CrateBodyBuildState<'_> {
    // Walk every known body, collecting local facts and lowering newly discovered nested bodies.
    // This is a worklist rather than recursive descent: collecting one body can append nested
    // fn/const/static bodies, and the loop visits those appended bodies before resolution starts.
    pub(crate) fn materialize_body_local_items(
        &mut self,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
        task_source: BodyTaskSource<'_>,
        mut body_ref_for_nested: impl FnMut(LoweredBodyTask) -> anyhow::Result<BodyRef>,
    ) -> anyhow::Result<()> {
        self.body_local_items.clear();
        // `body_local_items` is the cursor into `crate_bodies`: each collected slot means that
        // body has its local DefMap/item store ready. Nested lowering may extend `crate_bodies`,
        // so the loop stops only once every appended body has been collected too.
        let cargo_target = def_map
            .package(self.crate_ref.package)?
            .crate_data(self.crate_ref.crate_id)
            .context("semantic crate should have definition data")?
            .cargo_target();
        let parse_target = self.parse_package.target(cargo_target).with_context(|| {
            format!(
                "while attempting to fetch parsed target {:?} for nested body lowering",
                self.crate_ref.crate_id,
            )
        })?;
        let cfg = CfgEvaluator::new(
            self.parse_package.cfg_options(),
            parse_target.enables_test_cfg(),
        );
        let mut macro_expansion = BodyMacroExpansion::new(self.parse_package, def_map, cfg);
        while self.body_local_items.len() < self.crate_bodies.bodies().len() {
            let body = self.body_local_items.next_id();
            let body_ref = self.body_ref(body);
            let items = self
                .collect_body_local_items(body, def_map, semantic_ir)
                .context("collect body-local declarations")?;
            if let Some((owner, owner_module)) = Self::request_root_owner_context(
                body_ref,
                self.crate_bodies.bodies()[body].body(),
                &items,
            )? {
                self.crate_bodies.bodies_mut()[body].set_owner_context(owner, owner_module);
            }
            let body_data = self.crate_bodies.bodies()[body].body();
            let nested_tasks = Self::nested_body_tasks(
                body_ref,
                body_data.source(),
                body_data.owner(),
                body_data.fallback_module(),
                items.def_map(),
                items.item_store(),
                &self.cancellation,
            )
            .context("collect nested body tasks")?;
            let allocated = self.body_local_items.alloc(Some(items));
            debug_assert_eq!(allocated, body);

            if !nested_tasks.is_empty() {
                BodyTaskLowering::new(
                    task_source,
                    &mut self.crate_bodies,
                    cfg,
                    self.interner,
                    &self.cancellation,
                )
                .lower_tasks(&nested_tasks, &mut macro_expansion)?
                .into_iter()
                .try_for_each(|lowered| {
                    let body_ref = body_ref_for_nested(lowered)?;
                    anyhow::ensure!(
                        body_ref.crate_ref == self.crate_ref,
                        "nested body identity belongs to a different crate",
                    );
                    anyhow::ensure!(
                        !self.body_slots.contains_key(&body_ref),
                        "nested body identity {:?} was allocated more than once",
                        body_ref.body,
                    );
                    let allocated = self.body_refs.alloc(body_ref);
                    anyhow::ensure!(
                        allocated == lowered.body,
                        "nested body identity worklist is not aligned with lowered bodies",
                    );
                    anyhow::ensure!(
                        self.body_slots.insert(body_ref, allocated).is_none(),
                        "nested body identity was already present in the worklist",
                    );
                    Ok(())
                })?;
            }
        }

        Ok(())
    }

    /// Find the semantic owner assigned to a new or changed request-local root.
    ///
    /// Its provisional owner uses this body's origin so it can be recognized before collection.
    /// Nested declarations use their parent body's origin instead and therefore do not enter this
    /// path. Matching by declaration family and span then attaches the exact item lowered from the
    /// current header.
    fn request_root_owner_context(
        body_ref: BodyRef,
        body: &crate::BodyData,
        items: &BodyLocalItems,
    ) -> anyhow::Result<Option<(BodyOwner, ModuleRef)>> {
        if body.owner().declaration().origin() != DefMapRef::Body(body_ref) {
            return Ok(None);
        }

        let source = body.source();
        let (owner, owner_module) = match body.owner() {
            BodyOwner::Function(_) => {
                let mut matches =
                    items
                        .item_store()
                        .functions_with_refs()
                        .filter_map(|(function, data)| {
                            (data.source.file_id == source.file_id && data.span == source.span)
                                .then_some((BodyOwner::Function(function), data.owner))
                        });
                let Some((owner, item_owner)) = matches.next() else {
                    anyhow::bail!(
                        "request-local body root has no function declaration in its item store"
                    );
                };
                anyhow::ensure!(
                    matches.next().is_none(),
                    "request-local body root has more than one function declaration in its item store",
                );
                let owner_module =
                    Self::owner_module_for_body_item_owner(items.item_store(), item_owner)
                        .context("request-local function root has no module in its item store")?;
                (owner, owner_module)
            }
            BodyOwner::Const(_) => {
                let origin = DefMapRef::Body(body_ref);
                let mut matches =
                    items
                        .item_store()
                        .consts()
                        .iter_with_ids()
                        .filter_map(|(id, data)| {
                            (data.source.file_id == source.file_id && data.span == source.span)
                                .then_some((BodyOwner::Const(ConstRef { origin, id }), data.owner))
                        });
                let Some((owner, item_owner)) = matches.next() else {
                    anyhow::bail!(
                        "request-local body root has no const declaration in its item store"
                    );
                };
                anyhow::ensure!(
                    matches.next().is_none(),
                    "request-local body root has more than one const declaration in its item store",
                );
                let owner_module =
                    Self::owner_module_for_body_item_owner(items.item_store(), item_owner)
                        .context("request-local const root has no module in its item store")?;
                (owner, owner_module)
            }
            BodyOwner::Static(_) => {
                let origin = DefMapRef::Body(body_ref);
                let mut matches =
                    items
                        .item_store()
                        .statics()
                        .iter_with_ids()
                        .filter_map(|(id, data)| {
                            (data.source.file_id == source.file_id && data.span == source.span)
                                .then_some((
                                    BodyOwner::Static(StaticRef { origin, id }),
                                    data.owner,
                                ))
                        });
                let Some((owner, owner_module)) = matches.next() else {
                    anyhow::bail!(
                        "request-local body root has no static declaration in its item store"
                    );
                };
                anyhow::ensure!(
                    matches.next().is_none(),
                    "request-local body root has more than one static declaration in its item store",
                );
                (owner, owner_module)
            }
        };
        Ok(Some((owner, owner_module)))
    }

    // Collects the local items within a single already-lowered body.
    #[rg_std::cancelable("body declaration worklist", token = self.cancellation)]
    fn collect_body_local_items(
        &self,
        body: BodyId,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
    ) -> anyhow::Result<BodyLocalItems> {
        let body_ref = self.body_ref(body);
        let body = self.crate_bodies.bodies()[body].body();

        // Finalization can see previously collected body-local DefMaps. This is what lets nested
        // bodies import names from the body scope that declared them.
        let source = BodyBuildQuerySource::new(
            def_map,
            semantic_ir,
            self.crate_ref,
            &self.body_slots,
            &self.body_local_items,
        );
        LocalItemSource::for_body(body)
            .collect(body_ref, source, &self.cancellation)
            .context("collect body-local declarations")
    }

    fn nested_body_tasks(
        body_ref: BodyRef,
        body_source: BodySource,
        body_owner: BodyOwner,
        fallback_module: ModuleRef,
        def_map: &DefMap,
        item_store: &ItemStore,
        cancellation: &rg_std::CancellationToken,
    ) -> anyhow::Result<Vec<BodyLoweringTask>> {
        let origin = DefMapRef::Body(body_ref);
        let mut tasks = Vec::new();

        // Associated items share the function/const arenas with module items. Their body still
        // belongs to the associated item, but type lookup starts from the owning impl/trait module.
        for (function_ref, function_data) in item_store.functions_with_refs() {
            rg_std::check_cancel!(cancellation, "nested body task");
            if function_ref.origin != origin {
                continue;
            }
            if body_owner == BodyOwner::Function(function_ref) {
                continue;
            }
            if !Self::source_is_nested_in_body(
                body_source,
                function_data.source.file_id,
                function_data.span,
            ) {
                continue;
            }
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
                current_root_items: CurrentRootItems::None,
                owner_module,
                fallback_module,
                file_id: function_data.source.file_id,
                span: function_data.span,
            });
        }

        for (const_id, const_data) in item_store.consts().iter_with_ids() {
            rg_std::check_cancel!(cancellation, "nested body task");
            let const_ref = ConstRef {
                origin,
                id: const_id,
            };
            if body_owner == BodyOwner::Const(const_ref) {
                continue;
            }
            if !Self::source_is_nested_in_body(
                body_source,
                const_data.source.file_id,
                const_data.span,
            ) {
                continue;
            }
            let Some(owner_module) =
                Self::owner_module_for_body_item_owner(item_store, const_data.owner)
            else {
                continue;
            };
            tasks.push(BodyLoweringTask {
                owner: BodyOwner::Const(const_ref),
                current_root_items: CurrentRootItems::None,
                owner_module,
                fallback_module,
                file_id: const_data.source.file_id,
                span: const_data.span,
            });
        }

        // Foreign statics have no initializer to lower. Unlike functions, their declaration data
        // has no `has_body` bit, so the retained extern-block owner carries that distinction.
        for (static_id, static_data) in item_store.statics().iter_with_ids() {
            rg_std::check_cancel!(cancellation, "nested body task");
            let static_ref = StaticRef {
                origin,
                id: static_id,
            };
            if body_owner == BodyOwner::Static(static_ref) {
                continue;
            }
            if !Self::source_is_nested_in_body(
                body_source,
                static_data.source.file_id,
                static_data.span,
            ) {
                continue;
            }
            if def_map
                .foreign_block(static_data.local_def.local_def)
                .is_some()
            {
                continue;
            }
            tasks.push(BodyLoweringTask {
                owner: BodyOwner::Static(static_ref),
                current_root_items: CurrentRootItems::None,
                owner_module: static_data.owner,
                fallback_module,
                file_id: static_data.source.file_id,
                span: static_data.span,
            });
        }

        tasks.sort_by_key(|task| (task.file_id.0, task.span.start, task.span.end));
        Ok(tasks)
    }

    /// Distinguish declarations written inside the selected body from contextual declarations.
    ///
    /// For a current method, the temporary item store also contains its enclosing impl and sibling
    /// signatures. Those declarations participate in lookup, but their bodies are outside the
    /// selected method and must not extend this request's body worklist.
    fn source_is_nested_in_body(
        body_source: BodySource,
        item_file: FileId,
        item_span: Span,
    ) -> bool {
        body_source.file_id == item_file
            && body_source.span.contains_span(item_span)
            && body_source.span != item_span
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
