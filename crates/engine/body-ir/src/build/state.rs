//! Crate-local mutable state used while Body IR resolution is assembled.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use anyhow::Context as _;
use rg_arena::Arena;
use rg_cfg_eval::CfgEvaluator;
use rg_def_map::{DefMap, DefMapReadTxn};
use rg_ir_model::{
    BodyId, BodyRef, ConstRef, CrateRef, DefMapRef, ItemOwner, ModuleRef, StaticRef,
};
use rg_semantic_ir::{
    CrateItemQuery, ItemLookupQuery, ItemLookupQueryCache, ItemStore, SemanticIrReadTxn,
    TypePathResolution,
};
use rg_std::ExpectedUnique;
use rg_text::NameInterner;
use rg_ty::{TraitSelectionDeclarationCache, TraitSelectionSession};

use crate::{
    BodyFacts, BodyLocalItems, BodyOwner, CrateBodies, CurrentBody,
    resolution::{BodyResolutionContext, BodyResolutionPass},
};

use super::{
    body_def_map::BodyDefMapCollector,
    body_item_store::BodyItemStoreCollector,
    lower::{
        BodyLoweringTask, BodyMacroExpansion, BodyTaskLowering, BodyTaskSource, LoweredBodyTask,
        LoweredCrateBodies,
    },
    pattern_binding::PatternBindingMaterializationPass,
    query_source::BodyBuildQuerySource,
};

// Phase logs explain a slow crate-level build without emitting one event for every normal phase.
const SLOW_CRATE_RESOLUTION_PHASE: Duration = Duration::from_secs(1);
// Bodies are the highest-cardinality build unit. Construct their diagnostic record only after the
// timer crosses this threshold; a normal body does not create a tracing span or event.
const SLOW_BODY_RESOLUTION: Duration = Duration::from_secs(1);

/// Semantic stages shared by saved crate builds and selected current-body builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BodySemanticStage {
    ImplHeaders,
    PatternBindings,
    Bodies,
}

/// Time spent in each shared semantic stage.
pub(super) struct BodySemanticTimings {
    pub(super) impl_headers: Duration,
    pub(super) pattern_bindings: Duration,
    pub(super) bodies: Duration,
}

/// Coordinates all body-local facts needed to resolve one crate's bodies.
pub(super) struct CrateBodyBuildState<'crate_data> {
    crate_ref: CrateRef,
    parse_package: &'crate_data rg_parse::Package,
    crate_bodies: LoweredCrateBodies,
    /// Project identities for the bodies in the temporary worklist.
    ///
    /// Saved builds use the same ID for both arenas. A selected current build may reuse one saved
    /// identity or allocate a request-only identity, so its worklist slot cannot stand in for a
    /// `BodyRef`.
    body_refs: Arena<BodyId, BodyRef>,
    /// Worklist slot for each project or request-only body identity.
    body_slots: HashMap<BodyRef, BodyId>,
    body_facts: Arena<BodyId, BodyFacts>,
    body_local_items: Arena<BodyId, Option<BodyLocalItems>>,
    interner: &'crate_data mut NameInterner,
}

impl<'crate_data> CrateBodyBuildState<'crate_data> {
    pub(super) fn new(
        crate_ref: CrateRef,
        parse_package: &'crate_data rg_parse::Package,
        crate_bodies: LoweredCrateBodies,
        interner: &'crate_data mut NameInterner,
    ) -> Self {
        let mut body_refs = Arena::with_capacity(crate_bodies.bodies().len());
        let mut body_slots = HashMap::with_capacity(crate_bodies.bodies().len());
        for (body, _) in crate_bodies.bodies().iter_with_ids() {
            let body_ref = BodyRef { crate_ref, body };
            let allocated = body_refs.alloc(body_ref);
            debug_assert_eq!(allocated, body);
            body_slots.insert(body_ref, body);
        }
        Self {
            crate_ref,
            parse_package,
            crate_bodies,
            body_refs,
            body_slots,
            body_facts: Arena::new(),
            body_local_items: Arena::new(),
            interner,
        }
    }

    /// Start the same semantic pipeline from request-local lowered roots.
    pub(super) fn for_current(
        crate_ref: CrateRef,
        parse_package: &'crate_data rg_parse::Package,
        crate_bodies: LoweredCrateBodies,
        body_refs: Vec<BodyRef>,
        interner: &'crate_data mut NameInterner,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            crate_bodies.bodies().len() == body_refs.len(),
            "current body worklist and identity list have different lengths",
        );
        anyhow::ensure!(
            body_refs.iter().all(|body| body.crate_ref == crate_ref),
            "current body identity belongs to a different crate",
        );
        anyhow::ensure!(
            body_refs.iter().enumerate().all(|(index, body)| {
                !body_refs[..index].iter().any(|previous| previous == body)
            }),
            "current body identity was assigned to more than one root",
        );
        let body_slots = body_refs
            .iter()
            .copied()
            .enumerate()
            .map(|(slot, body_ref)| (body_ref, BodyId(slot)))
            .collect();
        Ok(Self {
            crate_ref,
            parse_package,
            crate_bodies,
            body_refs: Arena::from_vec(body_refs),
            body_slots,
            body_facts: Arena::new(),
            body_local_items: Arena::new(),
            interner,
        })
    }

    /// Resolve one lowered crate through a visibility-scoped semantic lookup query.
    ///
    /// The crate gets its own use-site visibility and solver answers. Dependency candidate
    /// composition and canonical crate declarations are reused from caches owned by the
    /// surrounding project build.
    pub(super) fn resolve(
        mut self,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
        declarations: &TraitSelectionDeclarationCache,
        item_lookup_cache: &ItemLookupQueryCache,
    ) -> anyhow::Result<CrateBodies> {
        let span = tracing::debug_span!(
            "body_ir_crate_resolution",
            rg.crate_id = self.crate_ref.crate_id.0,
        );
        let _entered = span.enter();
        let resolution_started = Instant::now();

        // Before resolving bodies on the expr level, we need to collect
        // the items declared within the body, and we need to match `impl`
        // blocks to their corresponding `Self` types.
        let phase_started = Instant::now();
        let crate_ref = self.crate_ref;
        self.materialize_body_local_items(
            def_map,
            semantic_ir,
            BodyTaskSource::Saved(self.parse_package),
            |lowered| {
                Ok(BodyRef {
                    crate_ref,
                    body: lowered.body,
                })
            },
        )?;
        let elapsed = phase_started.elapsed();
        let body_local_items_ms = elapsed.as_millis();
        if elapsed >= SLOW_CRATE_RESOLUTION_PHASE {
            tracing::debug!(
                phase = "body_local_items",
                elapsed_ms = elapsed.as_millis(),
                body_count = self.crate_bodies.bodies().len(),
                "slow Body IR crate resolution phase"
            );
        }

        // Build the visibility query before any body query starts. Declaration-local indexes live
        // in Semantic IR; items declared inside a body remain a separate overlay through
        // `BodyBuildQuerySource`.
        let phase_started = Instant::now();
        let crate_items = CrateItemQuery::new(def_map, semantic_ir, self.crate_ref);
        let item_lookup_query = ItemLookupQuery::build_with_cache(&crate_items, item_lookup_cache)?;
        let elapsed = phase_started.elapsed();
        let item_lookup_query_ms = elapsed.as_millis();
        if elapsed >= SLOW_CRATE_RESOLUTION_PHASE {
            tracing::debug!(
                phase = "item_lookup_query",
                elapsed_ms = elapsed.as_millis(),
                "slow Body IR crate resolution phase"
            );
        }
        let trait_selection =
            TraitSelectionSession::new_with_declaration_cache(self.crate_ref, declarations.clone());
        let semantic_timings = self.resolve_semantics(
            def_map,
            semantic_ir,
            &item_lookup_query,
            &trait_selection,
            |_| Ok(()),
        )?;
        let body_local_impl_headers_ms = semantic_timings.impl_headers.as_millis();
        let pattern_bindings_ms = semantic_timings.pattern_bindings.as_millis();
        let bodies_ms = semantic_timings.bodies.as_millis();

        // Finalize the build state, e.g. associate each body with its corresponding
        // defmap/item store.
        let body_count = self.crate_bodies.bodies().len();
        let finish_started = Instant::now();
        let bodies = self.finish();
        let finish_ms = finish_started.elapsed().as_millis();
        tracing::trace!(
            body_count,
            body_local_items_ms,
            item_lookup_query_ms,
            body_local_impl_headers_ms,
            pattern_bindings_ms,
            bodies_ms,
            finish_ms,
            total_ms = resolution_started.elapsed().as_millis(),
            "Body IR crate resolution phases finished"
        );
        Ok(bodies)
    }

    // Walk every known body, collecting local facts and lowering newly discovered nested bodies.
    // This is a worklist rather than recursive descent: collecting one body can append nested
    // fn/const/static bodies, and the loop visits those appended bodies before resolution starts.
    pub(super) fn materialize_body_local_items(
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
            let items = self.collect_body_local_items(body, def_map, semantic_ir)?;
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
                body_data.owner(),
                body_data.fallback_module(),
                items.def_map(),
                items.item_store(),
            );
            let allocated = self.body_local_items.alloc(Some(items));
            debug_assert_eq!(allocated, body);

            if !nested_tasks.is_empty() {
                BodyTaskLowering::new(task_source, &mut self.crate_bodies, cfg, self.interner)
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
        let def_map = BodyDefMapCollector::new(body_ref, body)
            .collect()
            .finalize(source)?;
        let item_store = BodyItemStoreCollector::new(body, &def_map).collect();

        Ok(BodyLocalItems::new(def_map, item_store))
    }

    fn nested_body_tasks(
        body_ref: BodyRef,
        body_owner: BodyOwner,
        fallback_module: ModuleRef,
        def_map: &DefMap,
        item_store: &ItemStore,
    ) -> Vec<BodyLoweringTask> {
        let origin = DefMapRef::Body(body_ref);
        let mut tasks = Vec::new();

        // Associated items share the function/const arenas with module items. Their body still
        // belongs to the associated item, but type lookup starts from the owning impl/trait module.
        for (function_ref, function_data) in item_store.functions_with_refs() {
            if function_ref.origin != origin {
                continue;
            }
            if body_owner == BodyOwner::Function(function_ref) {
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
                request_root: false,
                owner_module,
                fallback_module,
                file_id: function_data.source.file_id,
                span: function_data.span,
            });
        }

        for (const_id, const_data) in item_store.consts().iter_with_ids() {
            let const_ref = ConstRef {
                origin,
                id: const_id,
            };
            if body_owner == BodyOwner::Const(const_ref) {
                continue;
            }
            let Some(owner_module) =
                Self::owner_module_for_body_item_owner(item_store, const_data.owner)
            else {
                continue;
            };
            tasks.push(BodyLoweringTask {
                owner: BodyOwner::Const(const_ref),
                request_root: false,
                owner_module,
                fallback_module,
                file_id: const_data.source.file_id,
                span: const_data.span,
            });
        }

        // Foreign statics have no initializer to lower. Unlike functions, their declaration data
        // has no `has_body` bit, so the retained extern-block owner carries that distinction.
        for (static_id, static_data) in item_store.statics().iter_with_ids() {
            let static_ref = StaticRef {
                origin,
                id: static_id,
            };
            if body_owner == BodyOwner::Static(static_ref) {
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
                request_root: false,
                owner_module: static_data.owner,
                fallback_module,
                file_id: static_data.source.file_id,
                span: static_data.span,
            });
        }

        tasks.sort_by_key(|task| (task.file_id.0, task.span.text.start, task.span.text.end));
        tasks
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

    /// Run the semantic stages in the same order for saved and current worklists.
    ///
    /// Lowering and body-local item discovery decide which bodies belong to the worklist. From
    /// this point onward both build modes use exactly these stages: impl headers first, ambiguous
    /// pattern bindings next, then expression facts.
    pub(super) fn resolve_semantics(
        &mut self,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
        item_lookup_query: &ItemLookupQuery<'_>,
        trait_selection: &TraitSelectionSession,
        mut checkpoint: impl FnMut(BodySemanticStage) -> anyhow::Result<()>,
    ) -> anyhow::Result<BodySemanticTimings> {
        let started = Instant::now();
        self.resolve_body_local_impl_headers(
            def_map,
            semantic_ir,
            item_lookup_query,
            trait_selection,
        )?;
        let impl_headers = started.elapsed();
        Self::report_slow_semantic_stage("body_local_impl_headers", impl_headers, None);
        checkpoint(BodySemanticStage::ImplHeaders)
            .context("check body work after resolving body-local impl headers")?;

        let started = Instant::now();
        self.materialize_pattern_bindings(
            def_map,
            semantic_ir,
            item_lookup_query,
            trait_selection,
        )?;
        let pattern_bindings = started.elapsed();
        Self::report_slow_semantic_stage("pattern_bindings", pattern_bindings, None);
        checkpoint(BodySemanticStage::PatternBindings)
            .context("check body work after resolving pattern bindings")?;

        let started = Instant::now();
        self.resolve_bodies(def_map, semantic_ir, item_lookup_query, trait_selection)?;
        let bodies = started.elapsed();
        Self::report_slow_semantic_stage("bodies", bodies, Some(self.crate_bodies.bodies().len()));
        checkpoint(BodySemanticStage::Bodies).context("check body work after body resolution")?;

        Ok(BodySemanticTimings {
            impl_headers,
            pattern_bindings,
            bodies,
        })
    }

    fn report_slow_semantic_stage(
        phase: &'static str,
        elapsed: Duration,
        body_count: Option<usize>,
    ) {
        if elapsed < SLOW_CRATE_RESOLUTION_PHASE {
            return;
        }
        tracing::debug!(
            phase,
            elapsed_ms = elapsed.as_millis(),
            body_count,
            "slow Body IR crate resolution phase"
        );
    }

    // After body-local item collection, impl headers can be resolved against the body defmap and
    // item store. Both inherent and trait lookups depend on these precomputed header facts.
    fn resolve_body_local_impl_headers(
        &mut self,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
        item_lookup_query: &ItemLookupQuery<'_>,
        trait_selection: &TraitSelectionSession,
    ) -> anyhow::Result<()> {
        for (body_id, lowered_body) in self.crate_bodies.bodies().iter_with_ids() {
            let body_ref = self.body_ref(body_id);
            let body = lowered_body.body();
            let resolved_headers = {
                let Some(items) = self.body_local_items.get(body_id).and_then(Option::as_ref)
                else {
                    continue;
                };
                let impl_headers = items
                    .item_store()
                    .impls_with_refs()
                    .map(|(impl_ref, impl_data)| {
                        (
                            impl_ref.id,
                            impl_data.owner,
                            impl_data.self_ty.clone(),
                            impl_data.trait_ref.clone(),
                        )
                    })
                    .collect::<Vec<_>>();

                // In order to resolve body-local types, we need to be aware of
                // body-local items, so that's how we inject them.
                let source = BodyBuildQuerySource::new(
                    def_map,
                    semantic_ir,
                    self.crate_ref,
                    &self.body_slots,
                    &self.body_local_items,
                );
                let context = BodyResolutionContext::for_structure(
                    &source,
                    &source,
                    body_ref,
                    body,
                    item_lookup_query,
                    trait_selection.clone(),
                );
                let type_paths = context.type_path_query();
                let mut resolved_headers = Vec::new();
                for (impl_id, owner, self_ty, trait_ref) in impl_headers {
                    if owner.origin != DefMapRef::Body(body_ref) {
                        continue;
                    }

                    let Some(scope) = body.scope_for_module(body_ref, owner) else {
                        continue;
                    };

                    let ty = context.type_refs(scope).resolve(&self_ty)?;
                    let mut resolved_self_ty = ExpectedUnique::new();
                    for nominal in ty.as_adts() {
                        resolved_self_ty.push(nominal.def);
                    }

                    let mut resolved_trait_ref = ExpectedUnique::new();
                    if let Some(trait_ref) = trait_ref
                        && let Some(path) = trait_ref.as_def_map_path()
                        && let TypePathResolution::Trait(trait_ref) =
                            type_paths.resolve_in_scope(scope, &path)?
                    {
                        resolved_trait_ref.push(trait_ref);
                    }
                    resolved_headers.push((impl_id, resolved_self_ty, resolved_trait_ref));
                }
                resolved_headers
            };

            let Some(items) = self
                .body_local_items
                .get_mut(body_id)
                .and_then(Option::as_mut)
            else {
                continue;
            };
            for (impl_id, resolved_self_ty, resolved_trait_ref) in resolved_headers {
                let _ = items.set_impl_header_facts(impl_id, resolved_self_ty, resolved_trait_ref);
            }
        }

        Ok(())
    }

    fn materialize_pattern_bindings(
        &mut self,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
        item_lookup_query: &ItemLookupQuery<'_>,
        trait_selection: &TraitSelectionSession,
    ) -> anyhow::Result<()> {
        let source = BodyBuildQuerySource::new(
            def_map,
            semantic_ir,
            self.crate_ref,
            &self.body_slots,
            &self.body_local_items,
        );

        for (body_id, body) in self.crate_bodies.bodies_mut().iter_mut_with_ids() {
            let body_ref = self.body_refs[body_id];
            PatternBindingMaterializationPass::new(
                &source,
                &source,
                item_lookup_query,
                body_ref,
                body,
                trait_selection,
            )
            .materialize()?;
        }

        Ok(())
    }

    // For each body with resolved items, goes through the body content and finalizes the resolution,
    // e.g. resolves all the bindings and runs a fixed-point loop until no more information can be
    // extracted.
    fn resolve_bodies(
        &mut self,
        def_map: &DefMapReadTxn<'_>,
        semantic_ir: &SemanticIrReadTxn<'_>,
        item_lookup_query: &ItemLookupQuery<'_>,
        trait_selection: &TraitSelectionSession,
    ) -> anyhow::Result<()> {
        // Make the body resolution pass aware of body-local items.
        let source = BodyBuildQuerySource::new(
            def_map,
            semantic_ir,
            self.crate_ref,
            &self.body_slots,
            &self.body_local_items,
        );
        debug_assert!(self.body_facts.is_empty());

        for (body_id, body) in self.crate_bodies.bodies().iter_with_ids() {
            let body_ref = self.body_refs[body_id];
            let body = body.body();
            let body_source = body.source();
            let started = Instant::now();
            let facts = BodyResolutionPass::new(
                &source,
                &source,
                item_lookup_query,
                body_ref,
                body,
                trait_selection,
            )
            .resolve()?;
            let elapsed = started.elapsed();
            if elapsed >= SLOW_BODY_RESOLUTION {
                tracing::debug!(
                    body_id = body_id.0,
                    owner = ?body.owner(),
                    file_id = body_source.file_id.0,
                    path = ?self.parse_package.file_path(body_source.file_id),
                    span = ?body_source.span,
                    elapsed_ms = elapsed.as_millis(),
                    expression_count = body.exprs().len(),
                    binding_count = body.bindings().len(),
                    statement_count = body.statements().len(),
                    "slow body resolution"
                );
            }
            let allocated = self.body_facts.alloc(facts);
            debug_assert_eq!(allocated, body_id);
        }

        Ok(())
    }

    /// Finish request-local bodies without requiring their IDs to match worklist slots.
    pub(super) fn finish_current(mut self) -> anyhow::Result<Vec<CurrentBody>> {
        let body_count = self.crate_bodies.bodies().len();
        anyhow::ensure!(
            body_count == self.body_refs.len()
                && body_count == self.body_facts.len()
                && body_count == self.body_local_items.len(),
            "current Body IR worklist stages produced misaligned body data",
        );

        let body_refs = self.body_refs.into_vec();
        let bodies = self.crate_bodies.into_bodies().into_vec();
        let facts = self.body_facts.into_vec();
        let local_items = self
            .body_local_items
            .iter_mut()
            .map(|items| {
                items
                    .take()
                    .expect("every current body should have collected body-local items")
            })
            .collect::<Vec<_>>();

        Ok(body_refs
            .into_iter()
            .zip(bodies)
            .zip(facts)
            .zip(local_items)
            .map(|(((body_ref, body), facts), local_items)| {
                CurrentBody::new(body_ref, body.into_body(), facts, local_items)
            })
            .collect())
    }

    fn finish(mut self) -> CrateBodies {
        debug_assert!(
            self.body_refs
                .iter_with_ids()
                .all(|(slot, body_ref)| body_ref.body == slot)
        );
        let mut body_local_items = Arena::with_capacity(self.body_local_items.len());
        for (body, items) in self.body_local_items.iter_mut_with_ids() {
            let items = items
                .take()
                .expect("every built body should have collected body-local items");
            let allocated = body_local_items.alloc(items);
            debug_assert_eq!(allocated, body);
        }
        let coverage = self.crate_bodies.coverage();
        let bodies = Arena::from_vec(
            self.crate_bodies
                .into_bodies()
                .into_vec()
                .into_iter()
                .map(|body| body.into_body())
                .collect(),
        );

        CrateBodies::from_build(coverage, bodies, self.body_facts, body_local_items)
    }

    fn body_ref(&self, body: BodyId) -> BodyRef {
        self.body_refs[body]
    }
}
