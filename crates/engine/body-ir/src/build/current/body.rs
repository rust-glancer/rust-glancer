//! Builds Body IR for function, const, and static bodies from the text in the editor.
//!
//! We do not rebuild a project after every keystroke. Instead, this module finds the nearest
//! enclosing function, const, or static. A declaration with the same saved header keeps its saved
//! identity. A new or changed declaration instead gets a request-only item, including the
//! enclosing impl or trait header needed by an associated function or const. Either declaration
//! becomes the root of the same small body worklist used by saved builds.
//!
//! The saved project still supplies module declarations, traits, and crate-wide indexes. Locals,
//! expressions, scopes, body-local impls, and nested bodies come from the current text. For a body
//! inside an impl, the current impl header and sibling signatures are added only to that body's
//! temporary lookup context. They are not added to crate-wide indexes, so an unrelated body cannot
//! discover the unsaved members. If a root cannot be identified safely, we skip it rather than
//! attach current code to an unrelated declaration.

use std::time::Instant;

use crate::store::current::SelectedImpl;

use anyhow::Context as _;
use rg_cfg_eval::CfgEvaluator;
use rg_ir_model::{
    BodyRef, ConstId, ConstRef, CrateRef, DefMapRef, FunctionId, FunctionRef, ImplRef, ItemOwner,
    ModuleRef, StaticId, StaticRef, TraitDefRef,
};
use rg_parse::{CurrentSource, DeclarationAssociationIndex, FileId, Span};
use rg_semantic_ir::{CrateItemQuery, ItemLookupQuery, ItemLookupQueryCache, ItemStoreQuery};
use rg_std::ExpectedUnique;
use rg_text::NameInterner;
use rg_ty::TraitSelectionSession;

use crate::{
    BodyOwner, CrateBodiesCoverage, CurrentBody,
    build::state::{BodySemanticStage, CrateBodyBuildState},
};

use super::{
    CurrentSourceBuildCheckpoint, CurrentSourceSelection, CurrentSourceUnavailable,
    saved_identity::{SavedNestedBodyIndex, SavedRootOwnerIndex},
    syntax_owner::SyntaxBodyOwner,
};
use crate::build::lower::{
    BodyLoweringTask, BodyMacroExpansion, BodyTaskLowering, BodyTaskSource, CurrentRootItems,
    LoweredCrateBodies,
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
    parse_package: &'source rg_parse::Package,
    cfg: CfgEvaluator<'source>,
    def_map: &'source rg_def_map::DefMapReadTxn<'db>,
    semantic_ir: &'source rg_semantic_ir::SemanticIrReadTxn<'db>,
    saved_body_ir: &'source crate::BodyIrReadTxn<'db>,
    crate_ref: CrateRef,
    file: FileId,
    current_source: &'source CurrentSource,
    associations: &'source DeclarationAssociationIndex,
    item_lookup_cache: ItemLookupQueryCache,
    selection: CurrentSourceSelection,
    trait_selection: TraitSelectionSession,
}

impl<'source, 'db> CurrentBodyBuilder<'source, 'db> {
    /// Prepare current-body construction for one explicit selection policy.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        parse_package: &'source rg_parse::Package,
        cfg: CfgEvaluator<'source>,
        def_map: &'source rg_def_map::DefMapReadTxn<'db>,
        semantic_ir: &'source rg_semantic_ir::SemanticIrReadTxn<'db>,
        saved_body_ir: &'source crate::BodyIrReadTxn<'db>,
        crate_ref: CrateRef,
        file: FileId,
        current_source: &'source CurrentSource,
        associations: &'source DeclarationAssociationIndex,
        item_lookup_cache: ItemLookupQueryCache,
        selection: CurrentSourceSelection,
        trait_selection: TraitSelectionSession,
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
            item_lookup_cache,
            selection,
            trait_selection,
        }
    }

    /// Build request-local Body IR for the selected part of the editor text.
    ///
    /// Before ordinary body lowering can start, each selected body needs an owner and a `BodyRef`.
    /// Functions and initializers discovered inside that body need identities too. This method
    /// prepares those inputs, then runs the shared body worklist. The caller supplies new body ids
    /// when saved identities cannot be reused and receives checkpoints where cancelled work can
    /// stop.
    #[rg_std::cancelable("select current bodies", token = self.trait_selection.cancellation())]
    pub fn build(
        self,
        mut synthetic_body_ref: impl FnMut() -> anyhow::Result<BodyRef>,
        mut checkpoint: impl FnMut(CurrentSourceBuildCheckpoint) -> anyhow::Result<()>,
    ) -> anyhow::Result<CurrentBodyBuildOutcome> {
        let cancellation = self.trait_selection.cancellation().clone();
        let started = Instant::now();

        // 1. Parse the editor text and choose the syntax bodies requested by the cursor or range.
        // Selection deliberately stops at syntax: it does not yet decide which semantic
        // declaration, if any, owns each body.
        let parse_started = Instant::now();
        let current_parse = self
            .current_source
            .parse(self.parse_package.edition())
            .context("current source was not parsed for this package edition")?;
        let syntax_errors = current_parse.errors();
        let syntax = current_parse.tree();
        checkpoint(CurrentSourceBuildCheckpoint::SourceParsed)
            .context("check current-body work after source parsing")?;
        let current_owners = SyntaxBodyOwner::select(
            &syntax,
            self.current_source.text(),
            syntax_errors.as_slice(),
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
        let saved_items = self.semantic_ir.items(self.crate_ref)?;
        let saved_bodies = self.saved_body_ir.bodies(self.crate_ref, Some(self.file))?;
        let saved_owners = SavedRootOwnerIndex::new(saved_items, self.crate_ref, self.file);
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
                    let body_ref = match saved_bodies
                        .iter()
                        .find(|(_, body)| body.owner() == saved_owner)
                        .map(|(body_ref, _)| *body_ref)
                    {
                        Some(body_ref) => body_ref,
                        None => {
                            synthetic_body_ref().context("allocate request-only body identity")?
                        }
                    };
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
                        SyntaxBodyOwner::Function(_) => {
                            BodyOwner::Function(FunctionRef::new(origin, FunctionId(0)))
                        }
                        SyntaxBodyOwner::Const(_) => BodyOwner::Const(ConstRef {
                            origin,
                            id: ConstId(0),
                        }),
                        SyntaxBodyOwner::Static(_) => BodyOwner::Static(StaticRef {
                            origin,
                            id: StaticId(0),
                        }),
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
        let saved_nested_bodies = SavedNestedBodyIndex::new(
            self.saved_body_ir,
            self.crate_ref,
            self.file,
            &saved_bodies,
            roots.iter().map(|root| root.body_ref),
        )?;

        let crate_items = CrateItemQuery::new(self.def_map, self.semantic_ir, self.crate_ref);
        let item_lookup_query =
            ItemLookupQuery::build_with_cache(&crate_items, &self.item_lookup_cache, &cancellation)
                .context("build the current body's visible item lookup query")?;

        let mut interner = NameInterner::new();
        let task_source = BodyTaskSource::Current {
            package: self.parse_package,
            file: self.file,
            source: self.current_source,
        };
        let tasks = roots
            .iter()
            .map(|root| root.lowering_task(self.file))
            .collect::<Vec<_>>();

        // 4. Lower the chosen roots from editor syntax. This is the ordinary mechanical body
        // lowerer: it records expressions, patterns, and lexical scopes but does not resolve their
        // meaning yet.
        let lowering_started = Instant::now();
        let mut lowered =
            LoweredCrateBodies::with_coverage(CrateBodiesCoverage::files(vec![self.file]));
        let mut macro_expansion =
            BodyMacroExpansion::new(self.parse_package, self.def_map, self.cfg);
        let lowered_roots = BodyTaskLowering::new(
            task_source,
            &mut lowered,
            self.cfg,
            &mut interner,
            &cancellation,
        )
        .lower_tasks(&tasks, &mut macro_expansion)?;
        anyhow::ensure!(
            lowered_roots.len() == roots.len(),
            "an associated current body could not be lowered from its captured syntax",
        );
        let root_body_refs = lowered_roots
            .iter()
            .map(|lowered| {
                roots
                    .iter()
                    .find(|root| root.current_span == lowered.task.span)
                    .map(|root| root.body_ref)
                    .context("lowered current root has no associated body identity")
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let lowering_us = lowering_started.elapsed().as_micros();
        checkpoint(CurrentSourceBuildCheckpoint::BodyLowered)
            .context("check current-body work after root lowering")?;

        // 5. Collect declarations inside the lowered roots. This extends the same worklist with
        // nested functions and initializers. A uniquely associated nested declaration keeps its
        // saved identity; new or ambiguous declarations receive request-only identities.
        let local_items_started = Instant::now();
        let mut build = CrateBodyBuildState::for_current(
            self.crate_ref,
            self.parse_package,
            lowered,
            root_body_refs,
            &mut interner,
            cancellation.clone(),
        )?;
        build.materialize_body_local_items(
            self.def_map,
            self.semantic_ir,
            task_source,
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
        let local_items_us = local_items_started.elapsed().as_micros();
        checkpoint(CurrentSourceBuildCheckpoint::BodyLocalItemsCollected)
            .context("check current-body work after collecting body-local items")?;

        // 6. Run the normal semantic stages over the completed request-local worklist. Only after
        // impl headers, pattern bindings, and body names are resolved do we expose these bodies to
        // the analysis request.
        let semantic_timings = build.resolve_semantics(
            self.def_map,
            self.semantic_ir,
            &item_lookup_query,
            &self.trait_selection,
            |stage| {
                checkpoint(match stage {
                    BodySemanticStage::ImplHeaders => {
                        CurrentSourceBuildCheckpoint::ImplHeadersResolved
                    }
                    BodySemanticStage::PatternBindings => {
                        CurrentSourceBuildCheckpoint::PatternBindingsMaterialized
                    }
                    BodySemanticStage::Bodies => CurrentSourceBuildCheckpoint::BodyResolved,
                })
            },
        )?;
        let bodies = build.finish_current()?;

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
                let is_context = roots.iter().any(|root| {
                    root.body_ref == body.body_ref()
                        && matches!(
                            root.current_root_items,
                            CurrentRootItems::EnclosingImpl {
                                include_selected: false
                            }
                        )
                        && local.span.contains_span(root.current_span)
                });
                if !is_context {
                    complete_impls.push(SelectedImpl {
                        source: crate::BodySource::written(local.file_id, local.span),
                        impl_ref,
                    });
                }
            }
        }

        tracing::trace!(
            package = self.crate_ref.package.0,
            crate_id = self.crate_ref.crate_id.0,
            file_id = self.file.0,
            body_count = bodies.len(),
            unavailable_count = unavailable.len(),
            interned_name_count = interner.len(),
            parse_us,
            association_us,
            lowering_us,
            local_items_us,
            impl_headers_us = semantic_timings.impl_headers.as_micros(),
            pattern_bindings_us = semantic_timings.pattern_bindings.as_micros(),
            resolution_us = semantic_timings.bodies.as_micros(),
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
                (function.origin, data.owner)
            }
            BodyOwner::Const(konst) => {
                let Some(data) = items.const_data(konst)? else {
                    return Ok(None);
                };
                (konst.origin, data.owner)
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
