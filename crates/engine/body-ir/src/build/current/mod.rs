//! Prepare one frozen current-source context before editor queries begin.
//!
//! Source capture belongs to the project. This builder owns all temporary identities and decides
//! which saved declarations, current body contexts, and complete impls explain that source.
//! Mechanical body lowering and local-item collection remain shared with saved indexing.

mod body;
pub(crate) mod declaration;
mod saved_identity;
mod syntax_owner;
mod types;

use std::collections::HashMap;

use anyhow::Context as _;
use rg_cfg_eval::CfgEvaluator;
use rg_def_map::DefMapReadTxn;
use rg_ir_model::{BodyId, BodyRef, CrateRef, FileId, Span};
use rg_parse::{CurrentSource, DeclarationAssociationIndex, enclosing_inline_module_path};
use rg_semantic_ir::{ItemLookupQueryCache, SemanticIrReadTxn};
use rg_std::ExpectedUnique;
use rg_syntax::{AstNode as _, ast};
use rg_text::NameInterner;
use rg_ty::TraitSelectionSession;

use crate::store::current::{CurrentImplData, CurrentImplRole, SelectedImpl};
use crate::{BodyIrReadTxn, BodySource, BodySourceItems, CurrentSourceStore, ScopeData};

use self::{body::CurrentBodyBuilder, declaration::CurrentDeclarationBuilder};
use super::local_items::LocalItemSource;

pub use self::types::{
    CurrentSourceBuildCheckpoint, CurrentSourceSelection, CurrentSourceUnavailable,
};

/// Unavailable crate interpretations and body spans rebuilt for one current-source request.
///
/// A cursor on an impl header can be fully prepared without rebuilding a body, so an empty
/// body-span list does not by itself mean preparation was unavailable.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CurrentSourceBuildSummary {
    unavailable: Vec<(CrateRef, CurrentSourceUnavailable)>,
    rebuilt_body_spans: Vec<(CrateRef, FileId, Span)>,
}

impl CurrentSourceBuildSummary {
    pub fn is_complete(&self) -> bool {
        self.unavailable.is_empty()
    }

    pub fn rebuilt_body_spans(&self) -> &[(CrateRef, FileId, Span)] {
        &self.rebuilt_body_spans
    }

    pub fn unavailable(&self) -> &[(CrateRef, CurrentSourceUnavailable)] {
        &self.unavailable
    }
}

/// Construct one request's selected bodies and declarations using the same saved readers.
///
/// Each target contributes its own semantic interpretation of the captured source. The builder
/// owns temporary identities across all targets and releases unfinished work if preparation fails.
pub struct CurrentSourceBuilder<'request, 'db> {
    def_map: &'request DefMapReadTxn<'db>,
    semantic_ir: &'request SemanticIrReadTxn<'db>,
    saved_bodies: &'request BodyIrReadTxn<'db>,
    source: &'request CurrentSource,
    lookup_cache: ItemLookupQueryCache,
    cancellation: rg_std::CancellationToken,
    next_body_ids: HashMap<CrateRef, usize>,
    current: CurrentSourceStore,
    summary: CurrentSourceBuildSummary,
}

impl rg_std::Cancelable for CurrentSourceBuilder<'_, '_> {
    fn check_cancelled(&self, checkpoint: &'static str) -> Result<(), rg_std::Cancelled> {
        rg_std::Cancelable::check_cancelled(&self.cancellation, checkpoint)
    }
}

impl<'request, 'db> CurrentSourceBuilder<'request, 'db> {
    pub fn new(
        def_map: &'request DefMapReadTxn<'db>,
        semantic_ir: &'request SemanticIrReadTxn<'db>,
        saved_bodies: &'request BodyIrReadTxn<'db>,
        source: &'request CurrentSource,
        lookup_cache: ItemLookupQueryCache,
        cancellation: rg_std::CancellationToken,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            saved_bodies,
            source,
            lookup_cache,
            cancellation,
            next_body_ids: HashMap::new(),
            current: CurrentSourceStore::default(),
            summary: CurrentSourceBuildSummary::default(),
        }
    }

    /// Add one exact crate/file interpretation using the request's captured bytes.
    #[allow(clippy::too_many_arguments)]
    #[rg_std::cancelable("prepare current target")]
    pub fn prepare_target(
        &mut self,
        package: &rg_parse::Package,
        crate_ref: CrateRef,
        file: FileId,
        associations: &DeclarationAssociationIndex,
        source_changed: bool,
        selection: CurrentSourceSelection,
        trait_selection: TraitSelectionSession,
        mut checkpoint: impl FnMut(CurrentSourceBuildCheckpoint) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let trait_selection = trait_selection.with_cancellation(self.cancellation.clone());
        let mut checkpoint = |phase| {
            checkpoint(phase).context("observe current preparation")?;
            rg_std::check_cancel!(self.cancellation, "current preparation phase");
            Ok(())
        };
        // Bodies and declaration-only contexts use the same Cargo target's cfg policy.
        let cargo_target = self
            .def_map
            .package(crate_ref.package)
            .context("load current target definitions")?
            .crate_data(crate_ref.crate_id)
            .context("current crate has no definition data")?
            .cargo_target();
        let target = package
            .target(cargo_target)
            .context("current crate has no parse target")?;
        let cfg = CfgEvaluator::new(package.cfg_options(), target.enables_test_cfg());

        let ids = &mut self.next_body_ids;
        let saved_bodies = self.saved_bodies;
        let mut built = CurrentBodyBuilder::new(
            package,
            cfg,
            self.def_map,
            self.semantic_ir,
            saved_bodies,
            crate_ref,
            file,
            self.source,
            associations,
            self.lookup_cache.clone(),
            selection,
            trait_selection,
        )
        .build(
            || Self::allocate_body(ids, saved_bodies, crate_ref),
            &mut checkpoint,
        )
        .context("prepare selected current bodies")?;

        // A cursor's impl is a separate query requirement from its nearest expression body.
        // Preparing it here lets completion and actions remain read-only even for new impls.
        if let CurrentSourceSelection::AtOffset(offset) = selection {
            let parse = self
                .source
                .parse(package.edition())
                .context("current source was not parsed for the target edition")?;
            let mut candidates = Vec::new();
            for (index, node) in parse.tree().syntax().descendants().enumerate() {
                if index % 64 == 0 {
                    rg_std::check_cancel!(self.cancellation, "current declaration selection");
                }
                let Some(impl_) = ast::Impl::cast(node) else {
                    continue;
                };
                let contains_cursor = {
                    let span = Span::from_text_range(impl_.syntax().text_range());
                    if span.touches(offset) {
                        candidates.push(impl_);
                        continue;
                    }
                    // At `impl Service for Worker { $0`, the parser can end the impl at `{`.
                    // Whitespace after an unclosed member list still belongs to that impl. Read
                    // the captured syntax here; the completion marker must not enter semantics.
                    span.text.end <= offset
                        && offset as usize <= self.source.text().len()
                        && impl_
                            .assoc_item_list()
                            .is_some_and(|list| list.r_curly_token().is_none())
                        && self
                            .source
                            .text()
                            .get(span.text.end as usize..)
                            .is_some_and(|tail| tail.chars().all(char::is_whitespace))
                };
                if contains_cursor {
                    candidates.push(impl_);
                }
            }
            let selected = candidates
                .into_iter()
                .min_by_key(|impl_| impl_.syntax().text_range().len());
            if let Some(impl_) = selected {
                built
                    .unavailable
                    .retain(|reason| *reason != CurrentSourceUnavailable::NoBodyAtPosition);
                let span = Span::from_text_range(impl_.syntax().text_range());
                let source = BodySource::written(file, span);
                // Prefer the saved impl when its header still matches. A declaration can move in
                // editor text, so map it to its saved span before looking up its semantic identity.
                let mut saved_impl = ExpectedUnique::Empty;
                if let ExpectedUnique::One(saved_span) =
                    associations.saved_declaration_for_current(impl_.syntax())
                    && let Some(items) = self
                        .semantic_ir
                        .items(crate_ref)
                        .context("load saved impl items")?
                {
                    let def_map = self
                        .def_map
                        .def_map(crate_ref)
                        .context("load saved impl definitions")?
                        .context("saved impl has no definition map")?;
                    for (impl_ref, data) in items.impls_with_refs() {
                        rg_std::check_cancel!(self.cancellation, "saved impl identity");
                        let Some(local) = def_map.local_impl(data.local_impl.local_impl) else {
                            continue;
                        };
                        if local.file_id == file && local.span == saved_span {
                            saved_impl.push(impl_ref);
                        }
                    }
                }
                let saved_impl = match saved_impl {
                    ExpectedUnique::One(value) => Some(value),
                    _ => None,
                };
                let body_owns_cursor = built
                    .bodies
                    .iter()
                    .any(|body| body.source_span().touches(offset));
                // Some body contexts omit their selected saved member. Only complete contexts
                // can supply the member list when no saved impl matches.
                let complete_body_impl = built
                    .complete_impls
                    .iter()
                    .find(|prepared| prepared.source == source)
                    .map(|prepared| prepared.impl_ref);
                let mut selected_impl = saved_impl.or(complete_body_impl);

                // A complete current impl supplies source occurrences only when no body does.
                // Otherwise it remains private to member queries, avoiding duplicate generic IDs
                // in the ordinary signature scanner.
                if !body_owns_cursor || selected_impl.is_none() {
                    let module_path = enclosing_inline_module_path(impl_.syntax());
                    if let Some(fallback_module) = self
                        .def_map
                        .module_for_inline_path(crate_ref, file, &module_path)
                        .context("match current impl module to saved semantics")?
                    {
                        let body_ref = Self::allocate_body(
                            &mut self.next_body_ids,
                            self.saved_bodies,
                            crate_ref,
                        )
                        .context("allocate current impl identity")?;
                        let mut items = BodySourceItems::default();
                        let mut interner = NameInterner::new();
                        let root = CurrentDeclarationBuilder {
                            file,
                            line_index: self.source.line_index(),
                            cfg,
                            interner: &mut interner,
                            items: &mut items,
                            cancellation: &self.cancellation,
                        }
                        .impl_(&impl_, None)
                        .context("collect current impl declarations")?;
                        // The impl and its member signatures need one declaration scope. There
                        // is no expression body or binding scope to lower for this context.
                        let scopes = [ScopeData {
                            parent: None,
                            source_items: vec![root],
                            bindings: Vec::new(),
                        }];
                        let items = LocalItemSource {
                            source,
                            scopes: &scopes,
                            items: &items,
                        }
                        .collect(body_ref, self.def_map, &self.cancellation)
                        .context("collect current impl declarations")?;
                        let impl_ref = {
                            let mut impls = items.item_store().impls_with_refs();
                            let (impl_ref, _) =
                                impls.next().context("current impl store has no impl")?;
                            anyhow::ensure!(
                                impls.next().is_none(),
                                "current impl store has more than one impl"
                            );
                            impl_ref
                        };
                        selected_impl = selected_impl.or(Some(impl_ref));
                        self.current.impls.push(CurrentImplData {
                            source,
                            impl_ref,
                            fallback_module,
                            items,
                            role: if body_owns_cursor {
                                CurrentImplRole::Members
                            } else {
                                CurrentImplRole::Signature
                            },
                        });
                    } else {
                        built
                            .unavailable
                            .push(CurrentSourceUnavailable::NoSemanticRoot);
                    }
                }
                if let Some(impl_ref) = selected_impl {
                    self.current
                        .selected_impls
                        .push(SelectedImpl { source, impl_ref });
                }
            }
            checkpoint(CurrentSourceBuildCheckpoint::DeclarationsPrepared)
                .context("check current-source work after declaration preparation")?;
        }
        if source_changed {
            self.current.masked_files.insert((crate_ref, file));
        }
        self.summary.rebuilt_body_spans.extend(
            built
                .bodies
                .iter()
                .map(|body| (crate_ref, file, body.source_span())),
        );
        self.summary.unavailable.extend(
            built
                .unavailable
                .into_iter()
                .map(|reason| (crate_ref, reason)),
        );
        self.current.bodies.extend(built.bodies);
        Ok(())
    }

    /// No mutable preparation state reaches a query or a saved package artifact.
    pub fn finish(self) -> anyhow::Result<(CurrentSourceStore, CurrentSourceBuildSummary)> {
        rg_std::check_cancel!(self.cancellation, "finish current preparation");
        self.current
            .validate()
            .context("validate current-source storage")?;
        Ok((self.current, self.summary))
    }

    /// Bodies and declaration-only stores share the `DefMapRef::Body` namespace. Allocate from
    /// one sequence per crate, after its saved body ids, so each store has a distinct origin.
    fn allocate_body(
        next: &mut HashMap<CrateRef, usize>,
        saved: &BodyIrReadTxn<'_>,
        crate_ref: CrateRef,
    ) -> anyhow::Result<BodyRef> {
        let id = match next.get(&crate_ref) {
            Some(id) => *id,
            None => {
                saved
                    .first_synthetic_body_ref(crate_ref)
                    .context("read saved body identity limit")?
                    .body
                    .0
            }
        };
        next.insert(
            crate_ref,
            id.checked_add(1)
                .context("request-only body identity overflowed")?,
        );
        Ok(BodyRef {
            crate_ref,
            body: BodyId(id),
        })
    }
}
