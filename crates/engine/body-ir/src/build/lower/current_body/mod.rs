//! Builds Body IR for function, const, and static bodies from the text in the editor.
//!
//! We do not rebuild a project after every keystroke. Instead, this module finds the nearest
//! enclosing function, const, or static. A declaration with the same saved header keeps its saved
//! identity. A new or changed declaration instead gets a request-only item, including the
//! enclosing impl or trait header needed by an associated function or const. Either declaration
//! becomes the root of the same small body worklist used by saved builds.
//!
//! The saved project still supplies module declarations, traits, impls, and crate-wide indexes.
//! Locals, expressions, scopes, body-local impls, and nested bodies come from the current text. The
//! saved project supplies crate-wide facts. A request-only root is visible from its own body but is
//! not added to crate-wide indexes, so another function cannot discover an unsaved method. If a
//! root cannot be identified safely, we skip it rather than attach current code to an unrelated
//! declaration.

use std::time::Instant;

mod saved_identity;
mod syntax_owner;

use anyhow::Context as _;
use rg_cfg_eval::CfgEvaluator;
use rg_ir_model::{
    BodyRef, ConstId, ConstRef, CrateRef, DefMapRef, FunctionId, FunctionRef, ImplRef, ItemOwner,
    ModuleRef, StaticId, StaticRef, TraitDefRef,
};
use rg_parse::{CurrentSource, DeclarationAssociationIndex, FileId, Span, TextSpan};
use rg_semantic_ir::{CrateItemQuery, ItemLookupQuery, ItemLookupQueryCache, ItemStoreQuery};
use rg_std::ExpectedUnique;
use rg_text::NameInterner;
use rg_ty::TraitSelectionSession;

use crate::{
    BodyOwner, CrateBodiesCoverage, CurrentBody,
    build::state::{BodySemanticStage, CrateBodyBuildState},
};

use self::{
    saved_identity::{SavedNestedBodyIndex, SavedRootOwnerIndex},
    syntax_owner::SyntaxBodyOwner,
};
use super::{
    BodyLoweringTask, BodyMacroExpansion, BodyTaskLowering, BodyTaskSource, LoweredCrateBodies,
};

/// Why a body from the editor could not be attached to a saved or request-local declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::Display)]
pub enum CurrentBodyUnavailable {
    /// The cursor is outside every function, const, and static declaration that owns a body.
    #[display("the cursor is not inside a function, const, or static declaration with a body")]
    NoBodyAtPosition,
    /// The body has neither a saved owner nor a request-local declaration root.
    #[display("the current body has no usable semantic root")]
    NoSemanticRoot,
    /// More than one saved declaration has the same header and containing declarations.
    #[display("more than one saved semantic owner matches the current body")]
    AmbiguousSavedOwner,
}

/// Bodies rebuilt from one document, plus the bodies that could not be analyzed.
#[derive(Debug)]
pub struct CurrentBodyBuildOutcome {
    /// Bodies attached to a declaration and processed by the lowering and resolution pipeline.
    pub bodies: Vec<CurrentBody>,
    /// Reasons why the requested body selection could not be fully rebuilt.
    pub unavailable: Vec<CurrentBodyUnavailable>,
}

/// Points where current-body construction can stop after an expensive step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::Display)]
pub enum CurrentBodyBuildCheckpoint {
    #[display("after current source parsing")]
    SourceParsed,
    #[display("after current body owner association")]
    OwnerAssociated,
    #[display("after current body lowering")]
    BodyLowered,
    #[display("after current body-local item collection")]
    BodyLocalItemsCollected,
    #[display("after current body-local impl header resolution")]
    ImplHeadersResolved,
    #[display("after current pattern binding resolution")]
    PatternBindingsMaterialized,
    #[display("after current body resolution")]
    BodyResolved,
}

/// Coordinates a small Body IR rebuild for selected bodies in editor text.
///
/// This type does not provide a second lowering implementation. It chooses syntax roots, decides
/// which saved declaration each root still belongs to, and creates a request-local declaration
/// when no saved declaration matches. It then hands the roots to the same lowering and resolution
/// stages used by saved builds. The resulting bodies exist only for this analysis request.
pub struct CurrentBodyBuilder<'source, 'db> {
    parse_package: &'source rg_parse::Package,
    def_map: &'source rg_def_map::DefMapReadTxn<'db>,
    semantic_ir: &'source rg_semantic_ir::SemanticIrReadTxn<'db>,
    saved_body_ir: &'source crate::BodyIrReadTxn<'db>,
    crate_ref: CrateRef,
    file: FileId,
    current_source: &'source CurrentSource,
    associations: &'source DeclarationAssociationIndex,
    item_lookup_cache: ItemLookupQueryCache,
    selection: CurrentBodySelection,
    trait_selection: TraitSelectionSession,
}

/// How current syntax chooses the roots that enter one Body IR worklist.
///
/// A cursor selects the innermost body that touches it and includes parser recovery for unfinished
/// code. A range uses half-open overlap and may select several bodies. Keeping these policies
/// explicit avoids pretending that a cursor is just a very short range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentBodySelection {
    /// Select the innermost body-owning declaration that contains or can recover the cursor.
    AtOffset(u32),
    /// Select every body whose source has a strict half-open overlap with the range.
    IntersectingRange(TextSpan),
}

impl<'source, 'db> CurrentBodyBuilder<'source, 'db> {
    /// Prepare current-body construction for one explicit selection policy.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        parse_package: &'source rg_parse::Package,
        def_map: &'source rg_def_map::DefMapReadTxn<'db>,
        semantic_ir: &'source rg_semantic_ir::SemanticIrReadTxn<'db>,
        saved_body_ir: &'source crate::BodyIrReadTxn<'db>,
        crate_ref: CrateRef,
        file: FileId,
        current_source: &'source CurrentSource,
        associations: &'source DeclarationAssociationIndex,
        item_lookup_cache: ItemLookupQueryCache,
        selection: CurrentBodySelection,
    ) -> Self {
        Self {
            parse_package,
            def_map,
            semantic_ir,
            saved_body_ir,
            crate_ref,
            file,
            current_source,
            associations,
            item_lookup_cache,
            selection,
            trait_selection: TraitSelectionSession::new(crate_ref),
        }
    }

    /// Use the same crate-level trait solver cache as the rest of this analysis request.
    pub fn with_trait_selection(mut self, trait_selection: TraitSelectionSession) -> Self {
        self.trait_selection = trait_selection;
        self
    }

    /// Build request-local Body IR for the selected part of the editor text.
    ///
    /// Before ordinary body lowering can start, each selected body needs an owner and a `BodyRef`.
    /// Functions and initializers discovered inside that body need identities too. This method
    /// prepares those inputs, then runs the shared body worklist. The caller supplies new body ids
    /// when saved identities cannot be reused and receives checkpoints where cancelled work can
    /// stop.
    pub fn build(
        self,
        mut synthetic_body_ref: impl FnMut() -> anyhow::Result<BodyRef>,
        mut checkpoint: impl FnMut(CurrentBodyBuildCheckpoint) -> anyhow::Result<()>,
    ) -> anyhow::Result<CurrentBodyBuildOutcome> {
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
        checkpoint(CurrentBodyBuildCheckpoint::SourceParsed)
            .context("check current-body work after source parsing")?;
        let current_owners = SyntaxBodyOwner::select(
            &syntax,
            self.current_source.text(),
            syntax_errors.as_slice(),
            self.selection,
        );
        let parse_us = parse_started.elapsed().as_micros();
        if current_owners.is_empty() {
            let unavailable = matches!(self.selection, CurrentBodySelection::AtOffset(_))
                .then_some(CurrentBodyUnavailable::NoBodyAtPosition)
                .into_iter()
                .collect();
            return Ok(CurrentBodyBuildOutcome {
                bodies: Vec::new(),
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
            let association_started = Instant::now();
            let saved_root = self.find_saved_root(&selected_owner, &saved_owners);
            association_us += association_started.elapsed().as_micros();
            checkpoint(CurrentBodyBuildCheckpoint::OwnerAssociated)
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
                        include_current_declaration: false,
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
                        unavailable.push(CurrentBodyUnavailable::NoSemanticRoot);
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
                        include_current_declaration: true,
                    }
                }
                ExpectedUnique::Ambiguous => {
                    unavailable.push(CurrentBodyUnavailable::AmbiguousSavedOwner);
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
            ItemLookupQuery::build_with_cache(&crate_items, &self.item_lookup_cache)
                .context("build the current body's visible item lookup query")?;

        let cfg = self.cfg()?;
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
        let mut lowered = LoweredCrateBodies::with_coverage(CrateBodiesCoverage::Partial);
        let mut macro_expansion = BodyMacroExpansion::new(self.parse_package, self.def_map, cfg);
        let lowered_roots = BodyTaskLowering::new(task_source, &mut lowered, cfg, &mut interner)
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
        checkpoint(CurrentBodyBuildCheckpoint::BodyLowered)
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
        checkpoint(CurrentBodyBuildCheckpoint::BodyLocalItemsCollected)
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
                        CurrentBodyBuildCheckpoint::ImplHeadersResolved
                    }
                    BodySemanticStage::PatternBindings => {
                        CurrentBodyBuildCheckpoint::PatternBindingsMaterialized
                    }
                    BodySemanticStage::Bodies => CurrentBodyBuildCheckpoint::BodyResolved,
                })
            },
        )?;
        let bodies = build.finish_current()?;

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

    fn cfg(&self) -> anyhow::Result<CfgEvaluator<'source>> {
        let cargo_target = self
            .def_map
            .package(self.crate_ref.package)?
            .crate_data(self.crate_ref.crate_id)
            .context("saved semantic crate has no definition data")?
            .cargo_target();
        let target = self
            .parse_package
            .target(cargo_target)
            .context("saved parse package has no matching Cargo target")?;
        Ok(CfgEvaluator::new(
            self.parse_package.cfg_options(),
            target.enables_test_cfg(),
        ))
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
/// Saved roots already have declaration data in Semantic IR. A new or changed declaration instead
/// asks lowering to include its current item in the temporary body-local item store. Until that
/// store exists, its saved containing module is also its initial lookup context.
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
    /// Whether lowering must copy the current root declaration into its body-local item store.
    include_current_declaration: bool,
}

impl PreparedCurrentRoot {
    fn lowering_task(&self, file: FileId) -> BodyLoweringTask {
        BodyLoweringTask {
            owner: self.owner,
            request_root: self.include_current_declaration,
            owner_module: self.owner_module,
            fallback_module: self.fallback_module,
            file_id: file,
            span: self.current_span,
        }
    }
}
