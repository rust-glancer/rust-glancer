//! Builds Body IR for function, const, and static bodies from the text in the editor.
//!
//! We do not rebuild a project after every keystroke. Instead, this module finds the nearest
//! enclosing function, const, or static. An unchanged header keeps its saved identity; body facts
//! and source previews come from the selected text independently of that declaration.
//! A new or changed declaration gets a request-only item, including the enclosing impl or trait
//! header needed by an associated function or const. Either declaration becomes the root of the
//! same small body worklist used by saved builds.
//!
//! The saved project still supplies module declarations, traits, and crate-wide indexes. Locals,
//! expressions, scopes, body-local impls, and nested bodies come from the current text. For a body
//! inside an impl, the current impl header and sibling signatures are added only to that body's
//! temporary lookup context. They are not added to crate-wide indexes, so an unrelated body cannot
//! discover the unsaved members. If a root cannot be identified safely, we skip it rather than
//! attach current code to an unrelated declaration.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context as _;
use rg_cfg_eval::CfgEvaluator;
use rg_def_map::DefinitionReader;
use rg_ir_model::{
    BodyId, BodyKey, BodyRef, ConstId, ConstRef, CrateRef, DefMapRef, FileId, FunctionId,
    FunctionRef, ImplRef, ItemOwner, ItemStoreRef, ModuleRef, Span, StaticId, StaticRef,
    TraitDefRef,
};
use rg_item_tree::SourceLocations as _;
use rg_parse::{CurrentSource, DeclarationAssociationIndex, Package};
use rg_semantic_ir::{ItemLookupQuery, ItemReader, ItemStoreQuery};
use rg_std::{CancellationToken, ExpectedUnique};

use super::{
    CurrentSourceBuildCheckpoint, CurrentSourceSelection, CurrentSourceUnavailable,
    saved_identity::{SavedNestedBodyIndex, SavedRootOwnerIndex},
    syntax_owner::SyntaxBodyOwner,
};
use crate::{
    BodyOwner, BodyReader, BodySource, BodyStore, CurrentBody,
    build::{
        lower::{
            BodyLoweringTask, BodyMacroExpansion, BodyTaskLowering, BodyTaskSource,
            CurrentRootItems, LoweredBodies,
        },
        state::{BodySemanticStage, BodySemanticTimings, BodyStoreBuilder},
    },
    store::current::SelectedImpl,
};

pub(super) struct CurrentBodyBuildOutcome {
    pub(super) bodies: Vec<CurrentBody>,
    pub(super) complete_impls: Vec<SelectedImpl>,
    pub(super) unavailable: Vec<CurrentSourceUnavailable>,
}

/// Coordinates a small Body IR rebuild for selected bodies in editor text.
///
/// This type does not provide a second lowering implementation. It chooses syntax roots, decides
/// which saved declaration each root still belongs to, and creates a request-local declaration
/// when no saved declaration matches. It then hands the roots to the same lowering and resolution
/// stages used by saved builds. The resulting bodies exist only for this analysis request.
pub(super) struct CurrentBodyBuilder<'source, 'db> {
    parse_package: &'source Package,
    cfg: CfgEvaluator<'source>,
    def_map: &'source DefinitionReader<'db>,
    semantic_ir: &'source ItemReader<'db>,
    saved_body_ir: &'source BodyReader<'db>,
    crate_ref: CrateRef,
    file: FileId,
    current_source: &'source CurrentSource,
    associations: &'source DeclarationAssociationIndex,
    selection: CurrentSourceSelection,
    cancellation: CancellationToken,
}

impl<'source, 'db> CurrentBodyBuilder<'source, 'db> {
    /// Prepare current-body construction for one explicit selection policy.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        parse_package: &'source Package,
        cfg: CfgEvaluator<'source>,
        def_map: &'source DefinitionReader<'db>,
        semantic_ir: &'source ItemReader<'db>,
        saved_body_ir: &'source BodyReader<'db>,
        crate_ref: CrateRef,
        file: FileId,
        current_source: &'source CurrentSource,
        associations: &'source DeclarationAssociationIndex,
        selection: CurrentSourceSelection,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            parse_package,
            cfg,
            def_map,
            semantic_ir,
            saved_body_ir,
            crate_ref,
            file,
            current_source,
            associations,
            selection,
            cancellation,
        }
    }

    /// Build request-local Body IR for the selected part of the editor text.
    ///
    /// Before ordinary body lowering can start, each selected body needs an owner and a `BodyRef`.
    /// Functions and initializers discovered inside that body need identities too. This method
    /// prepares those inputs, then runs the shared body worklist. The caller supplies new body ids
    /// when saved identities cannot be reused and receives checkpoints where cancelled work can
    /// stop.
    #[rg_std::cancelable("select current bodies", token = self.cancellation)]
    pub fn build(
        self,
        mut synthetic_body_ref: impl FnMut() -> anyhow::Result<BodyRef>,
        mut checkpoint: impl FnMut(CurrentSourceBuildCheckpoint) -> anyhow::Result<()>,
    ) -> anyhow::Result<CurrentBodyBuildOutcome> {
        let cancellation = self.cancellation.clone();
        let started = Instant::now();

        // 1. Parse the editor text and choose the syntax bodies requested by the cursor or range.
        // Selection deliberately stops at syntax: it does not yet decide which semantic
        // declaration, if any, owns each body.
        let parse_started = Instant::now();
        let current_parse = self.current_source.parse();
        let syntax_errors = current_parse.parse_errors();
        let syntax = current_parse.tree();
        checkpoint(CurrentSourceBuildCheckpoint::SourceParsed)
            .context("check current-body work after source parsing")?;
        let current_owners = SyntaxBodyOwner::select(
            &syntax,
            self.current_source.text(),
            syntax_errors,
            self.selection,
            &cancellation,
        )
        .context("select current body roots")?;
        let parse_us = parse_started.elapsed().as_micros();
        if current_owners.is_empty() {
            let unavailable = matches!(self.selection, CurrentSourceSelection::AtOffset(_))
                .then_some(CurrentSourceUnavailable::NoBodyAtPosition)
                .into_iter()
                .collect();
            return Ok(CurrentBodyBuildOutcome {
                bodies: Vec::new(),
                complete_impls: Vec::new(),
                unavailable,
            });
        }

        // 2. Decide which declaration owns each selected body and where its worklist should start.
        // An unchanged declaration can reuse its saved owner. If no saved declaration matches, we
        // widen to the outermost current declaration so request-local parameters, `Self`, and
        // associated-item context remain available to nested bodies.
        let saved_items = self
            .semantic_ir
            .all_items(DefMapRef::Crate(self.crate_ref))?;
        let saved_owners = SavedRootOwnerIndex::new(&saved_items, self.file, self.saved_body_ir)?;
        let mut roots = Vec::<PreparedCurrentRoot>::new();
        let mut unavailable = Vec::new();
        let mut association_us = 0;
        for selected_owner in current_owners {
            rg_std::check_cancel!(cancellation, "current body roots");
            let association_started = Instant::now();
            let saved_root = self.find_saved_root(&selected_owner, &saved_owners);
            association_us += association_started.elapsed().as_micros();
            checkpoint(CurrentSourceBuildCheckpoint::OwnerAssociated)
                .context("check current-body work after owner association")?;
            let root = match saved_root {
                ExpectedUnique::One((current_owner, saved_owner)) => {
                    let current_span = Span::from_text_range(current_owner.syntax().text_range());
                    let body_ref = BodyKey {
                        krate: self.crate_ref,
                        owner: saved_owner.into(),
                    }
                    .body(BodyId(0));
                    let owner_module = self
                        .owner_module(saved_owner)?
                        .context("saved body owner has no module")?;
                    PreparedCurrentRoot {
                        current_span,
                        owner: saved_owner,
                        owner_module,
                        fallback_module: owner_module,
                        body_ref,
                        current_root_items: Self::current_root_items(&current_owner, false),
                    }
                }
                ExpectedUnique::Empty => {
                    let current_owner = selected_owner.outermost_body_owner();
                    let Some(fallback_module) = self
                        .def_map
                        .module_for_inline_path(
                            self.crate_ref,
                            self.file,
                            &current_owner.inline_module_path(),
                        )
                        .context("match current body module to saved semantics")?
                    else {
                        unavailable.push(CurrentSourceUnavailable::NoSemanticRoot);
                        continue;
                    };
                    let body_ref =
                        synthetic_body_ref().context("allocate request-only body identity")?;
                    let origin = DefMapRef::Body(body_ref);
                    let owner = match &current_owner {
                        SyntaxBodyOwner::Function(_) => BodyOwner::Function(FunctionRef::new(
                            ItemStoreRef::scope(origin),
                            FunctionId(0),
                        )),
                        SyntaxBodyOwner::Const(_) => {
                            BodyOwner::Const(ConstRef::new(ItemStoreRef::scope(origin), ConstId(0)))
                        }
                        SyntaxBodyOwner::Static(_) => {
                            BodyOwner::Static(StaticRef::new(origin, StaticId(0)))
                        }
                    };
                    PreparedCurrentRoot {
                        current_span: Span::from_text_range(current_owner.syntax().text_range()),
                        // The temporary item store assigns the final item id after it has collected
                        // the current declaration. Body lowering only needs the owner family; the
                        // real id is attached before semantic resolution starts.
                        owner,
                        // Collection allocates the final body-local module. The saved module is the
                        // correct context for macro expansion until that temporary store exists.
                        owner_module: fallback_module,
                        fallback_module,
                        body_ref,
                        current_root_items: Self::current_root_items(&current_owner, true),
                    }
                }
                ExpectedUnique::Ambiguous => {
                    unavailable.push(CurrentSourceUnavailable::AmbiguousSavedOwner);
                    continue;
                }
            };

            // A range can select both a nested body and an enclosing body. They may lead to the
            // same semantic root, which should enter the shared worklist only once.
            if roots
                .iter()
                .any(|prepared| prepared.current_span == root.current_span)
            {
                continue;
            }
            roots.push(root);
        }

        if roots.is_empty() {
            return Ok(CurrentBodyBuildOutcome {
                bodies: Vec::new(),
                complete_impls: Vec::new(),
                unavailable,
            });
        }

        // 3. Prepare the saved context needed after root lowering. The body worklist may discover
        // functions and initializers declared inside these roots. The nested-body index lets an
        // unchanged declaration keep its saved `BodyRef`; crate lookup continues to come from the
        // saved project rather than publishing request-local items globally.
        let saved_bodies =
            self.saved_body_ir
                .saved_prepared_bodies(self.crate_ref, self.file, &|owner| {
                    roots
                        .iter()
                        .any(|root| root.body_ref.root.owner() == Some(owner.owner))
                })?;
        let saved_nested_bodies = SavedNestedBodyIndex::new(
            self.saved_body_ir,
            self.crate_ref,
            self.file,
            &saved_bodies,
            roots.iter().map(|root| root.body_ref),
        )?;

        let item_lookup_query =
            ItemLookupQuery::build_from(*self.semantic_ir, self.crate_ref, &cancellation)
                .context("build the current body's visible item lookup query")?;
        let task_source = BodyTaskSource::Current {
            package: self.parse_package,
            file: self.file,
            source: self.current_source,
        };
        // Each selected outer owner has its own arena, just as it does in saved preparation.
        // Selecting a neighboring function must not renumber this owner's nested bodies.
        let mut bodies = Vec::new();
        let mut lowering_us = 0;
        let mut local_items_us = 0;
        let mut semantic_timings = BodySemanticTimings::default();
        let mut inference_time = Duration::ZERO;
        for root in &roots {
            let lowering_started = Instant::now();
            let mut lowered = LoweredBodies::default();
            let mut macro_expansion = BodyMacroExpansion::new(
                self.parse_package.edition(),
                self.def_map,
                self.cfg,
                cancellation.clone(),
            );
            let lowered_roots =
                BodyTaskLowering::new(task_source, &mut lowered, self.cfg, &cancellation)
                    .lower_tasks(&[root.lowering_task(self.file)], &mut macro_expansion)?;
            anyhow::ensure!(
                lowered_roots.len() == 1,
                "associated current body could not be lowered"
            );
            lowering_us += lowering_started.elapsed().as_micros();
            checkpoint(CurrentSourceBuildCheckpoint::BodyLowered)?;

            let local_items_started = Instant::now();
            let mut build =
                BodyStoreBuilder::new(root.body_ref, task_source, lowered, cancellation.clone());
            build.materialize_body_local_items(
                self.def_map,
                self.semantic_ir,
                self.cfg,
                |lowered| {
                    if let Some(saved_body) = self
                        .associations
                        .saved_declaration_for_current_span(lowered.task.span)
                        .and_then(|saved| saved_nested_bodies.body_ref_at(saved))
                    {
                        return Ok(saved_body);
                    }
                    synthetic_body_ref().context("allocate request-only nested body identity")
                },
            )?;
            local_items_us += local_items_started.elapsed().as_micros();
            checkpoint(CurrentSourceBuildCheckpoint::BodyLocalItemsCollected)?;
            let timings = build.prepare_semantics(
                self.def_map,
                self.semantic_ir,
                &item_lookup_query,
                |stage| {
                    checkpoint(match stage {
                        BodySemanticStage::ImplHeaders => {
                            CurrentSourceBuildCheckpoint::ImplHeadersResolved
                        }
                        BodySemanticStage::PatternBindings => {
                            CurrentSourceBuildCheckpoint::PatternBindingsMaterialized
                        }
                    })
                },
            )?;
            semantic_timings.impl_headers += timings.impl_headers;
            semantic_timings.pattern_bindings += timings.pattern_bindings;
            let (prepared, sources) = build.finish();

            // Compare the complete prepared owner, including its local declarations and nested
            // contexts. Matching only the selected expression tree could attach facts to other IDs.
            let saved_key = root.body_ref.saved_key();
            let saved = saved_key
                .map(|key| self.saved_body_ir.saved_prepared_owner(key))
                .transpose()?
                .flatten();
            let (prepared, facts) = if let (Some(key), Some(saved)) = (saved_key, saved)
                && *saved == prepared
            {
                (saved, self.saved_body_ir.saved_facts(key)?)
            } else {
                let started = Instant::now();
                let facts = prepared.infer(
                    self.def_map,
                    self.semantic_ir,
                    &item_lookup_query,
                    &cancellation,
                )?;
                inference_time += started.elapsed();
                (Arc::new(prepared), Arc::new(facts))
            };
            checkpoint(CurrentSourceBuildCheckpoint::BodyResolved)?;
            bodies.extend(CurrentBody::for_owner(BodyStore::new(
                prepared,
                facts,
                Arc::new(sources),
            )));
        }

        // A body-local impl is complete unless it is the copied enclosing context that omits
        // this root's saved member. Keep that distinction with construction, so later queries
        // never infer completeness from the mere presence of an ImplRef.
        let mut complete_impls = Vec::new();
        for body in &bodies {
            rg_std::check_cancel!(cancellation, "current body roots");
            for (impl_ref, impl_) in body.local_items().item_store().impls_with_refs() {
                rg_std::check_cancel!(cancellation, "current body roots");
                let local = body
                    .local_items()
                    .def_map()
                    .local_impl(impl_.local_impl.local_impl)
                    .context("collected impl has no local definition")?;
                let span = body
                    .view()
                    .sources()
                    .location(local.location)
                    .context("current impl occurrence is absent")?
                    .span;
                let is_context = roots.iter().any(|root| {
                    root.body_ref == body.body_ref()
                        && matches!(
                            root.current_root_items,
                            CurrentRootItems::EnclosingImpl {
                                include_selected: false
                            }
                        )
                        && span.contains_span(root.current_span)
                });
                if !is_context {
                    complete_impls.push(SelectedImpl {
                        source: BodySource::written(local.file_id, span),
                        impl_ref,
                    });
                }
            }
        }

        tracing::trace!(
            package = self.crate_ref.package.0,
            crate_id = self.crate_ref.crate_id.0,
            file_id = self.file.as_raw(),
            body_count = bodies.len(),
            unavailable_count = unavailable.len(),
            parse_us,
            association_us,
            lowering_us,
            local_items_us,
            impl_headers_us = semantic_timings.impl_headers.as_micros(),
            pattern_bindings_us = semantic_timings.pattern_bindings.as_micros(),
            resolution_us = inference_time.as_micros(),
            total_us = started.elapsed().as_micros(),
            "current body selection finished"
        );

        Ok(CurrentBodyBuildOutcome {
            bodies,
            complete_impls,
            unavailable,
        })
    }

    /// Find the saved crate body that should start the worklist.
    ///
    /// The selected function may itself be declared inside another function. Such declarations
    /// live in saved Body IR rather than crate Semantic IR, so they cannot be found in the root
    /// owner index. Walking outward finds a crate-level owner; the normal body-local worklist will
    /// reach the selected nested function again from editor text.
    fn find_saved_root(
        &self,
        selected: &SyntaxBodyOwner,
        saved_owners: &SavedRootOwnerIndex,
    ) -> ExpectedUnique<(SyntaxBodyOwner, BodyOwner)> {
        let mut result = ExpectedUnique::Empty;
        for syntax in selected.syntax().ancestors() {
            let Some(current_owner) = SyntaxBodyOwner::cast_with_body(syntax) else {
                continue;
            };
            match self
                .associations
                .saved_declaration_for_current(current_owner.syntax())
                .and_then(|saved_span| saved_owners.owner_at(saved_span))
            {
                ExpectedUnique::One(saved_owner) => {
                    return ExpectedUnique::One((current_owner, saved_owner));
                }
                ExpectedUnique::Ambiguous => result = ExpectedUnique::Ambiguous,
                ExpectedUnique::Empty => {}
            }
        }

        result
    }

    /// Choose how much declaration syntax the selected body's local store needs.
    ///
    /// Every current impl contributes its sibling signatures, including when the selected member
    /// keeps a saved identity. Other owners need current declaration data only when no saved
    /// identity could be reused.
    fn current_root_items(owner: &SyntaxBodyOwner, include_selected: bool) -> CurrentRootItems {
        if owner.belongs_to_impl() {
            return CurrentRootItems::EnclosingImpl { include_selected };
        }
        if include_selected {
            CurrentRootItems::Declaration
        } else {
            CurrentRootItems::None
        }
    }

    /// Find the module that gives a saved body root its normal name-resolution context.
    fn owner_module(&self, owner: BodyOwner) -> anyhow::Result<Option<ModuleRef>> {
        let items = ItemStoreQuery::new(self.semantic_ir);
        let (origin, item_owner) = match owner {
            BodyOwner::Function(function) => {
                let Some(data) = items.function_data(function)? else {
                    return Ok(None);
                };
                (function.store.origin, data.owner)
            }
            BodyOwner::Const(konst) => {
                let Some(data) = items.const_data(konst)? else {
                    return Ok(None);
                };
                (konst.store.origin, data.owner)
            }
            BodyOwner::Static(static_) => {
                return Ok(items.static_data(static_)?.map(|data| data.owner));
            }
        };

        Ok(match item_owner {
            ItemOwner::Module(module) => Some(module),
            ItemOwner::Trait(id) => items
                .trait_data(TraitDefRef { origin, id })?
                .map(|data| data.owner),
            ItemOwner::Impl(id) => items
                .impl_data(ImplRef { origin, id })?
                .map(|data| data.owner),
        })
    }
}

/// A selected root with the identity and module context needed by shared body lowering.
///
/// A saved root reuses its Semantic IR identity; when it belongs to an impl, lowering still copies
/// the current impl header and sibling signatures into its temporary lookup context. A new or
/// changed declaration also needs its own current item there. Until that store exists, the saved
/// containing module is the root's initial lookup context.
struct PreparedCurrentRoot {
    current_span: Span,
    owner: BodyOwner,
    /// The first module used for name lookup while lowering the body.
    ///
    /// A request-local root starts with its saved containing module. Body-local item collection
    /// replaces this with the temporary module that contains the current declaration.
    owner_module: ModuleRef,
    /// The saved containing module to try when body-local lookup does not find a name.
    fallback_module: ModuleRef,
    body_ref: BodyRef,
    /// Current declaration context copied into this body's temporary item store.
    current_root_items: CurrentRootItems,
}

impl PreparedCurrentRoot {
    fn lowering_task(&self, file: FileId) -> BodyLoweringTask {
        BodyLoweringTask {
            owner: self.owner,
            current_root_items: self.current_root_items,
            owner_module: self.owner_module,
            fallback_module: self.fallback_module,
            file_id: file,
            span: self.current_span,
        }
    }
}
