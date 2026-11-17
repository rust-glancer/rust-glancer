//! Saved body lookups needed while rebuilding bodies from editor text.
//!
//! There are two lookups because saved roots and saved nested bodies live in different stores.
//! A crate-level function, const, or static comes from Semantic IR. A function declared inside
//! another body comes from that body's local item store instead. Keeping both lookups here lets
//! the builder ask a simple question at each point: which saved owner or `BodyRef`, if any, is safe
//! to reuse for this current declaration?

use std::collections::HashMap;

use rg_ir_model::{
    BodyRef, CrateRef, DefMapRef, FileId, SemanticItemRef, Span, identity::DeclarationRef,
};
use rg_semantic_ir::{SemanticItemSource, SemanticItemView};
use rg_std::{ExpectedUnique, UniqueVec};

use crate::{BodyData, BodyOwner, BodyReader};

/// Finds the saved crate-level owner associated with a current body root.
///
/// Declaration association first maps current syntax to a saved declaration span. This index
/// finishes that lookup by mapping the saved span to its Semantic IR owner. We keep ambiguity
/// instead of choosing one owner, because attaching editor text to the wrong function would make
/// every later result look valid while referring to unrelated semantics.
pub(super) struct SavedRootOwnerIndex {
    owners: HashMap<Span, ExpectedUnique<BodyOwner>>,
}

impl SavedRootOwnerIndex {
    pub(super) fn new(
        items: &[SemanticItemView<'_>],
        file: FileId,
        bodies: &BodyReader<'_>,
    ) -> anyhow::Result<Self> {
        let mut owners = HashMap::<Span, ExpectedUnique<BodyOwner>>::new();
        for item in items {
            if item.source().file_id != file {
                continue;
            }
            let owner = match item.item() {
                SemanticItemRef::Function(function) => BodyOwner::Function(function),
                SemanticItemRef::Const(konst) => BodyOwner::Const(konst),
                SemanticItemRef::Static(static_ref) => BodyOwner::Static(static_ref),
                _ => continue,
            };
            if let Some(occurrence) = item.location() {
                let location = bodies.declaration_location(
                    item.item().origin().origin_crate(),
                    item.source(),
                    occurrence,
                )?;
                if location.editable {
                    owners.entry(location.span).or_default().push(owner);
                }
            }
        }
        Ok(Self { owners })
    }

    pub(super) fn owner_at(&self, span: Span) -> ExpectedUnique<BodyOwner> {
        self.owners.get(&span).cloned().unwrap_or_default()
    }
}

/// Finds saved identities for nested bodies reachable from the selected roots.
///
/// The normal body worklist rediscovers functions, consts, and statics declared inside another
/// body. When one of those declarations still matches saved syntax, reusing its `BodyRef` keeps
/// navigation and other identity-based facts connected to the saved project. We only walk bodies
/// below the selected roots, so one analysis request does not load every body-local item store
/// in the file.
pub(super) struct SavedNestedBodyIndex {
    bodies: HashMap<Span, ExpectedUnique<BodyRef>>,
}

impl SavedNestedBodyIndex {
    pub(super) fn new(
        saved_body_ir: &BodyReader<'_>,
        crate_ref: CrateRef,
        file: FileId,
        saved_bodies: &[(BodyRef, &BodyData)],
        roots: impl IntoIterator<Item = BodyRef>,
    ) -> anyhow::Result<Self> {
        let mut index = Self {
            bodies: HashMap::new(),
        };

        // Stored Body IR is keyed by `BodyRef`, while body-local item stores point at their nested
        // bodies through declaration identities. This lookup connects the two before we start the
        // selected-root walk below.
        let mut bodies_by_owner = HashMap::<DeclarationRef, ExpectedUnique<BodyRef>>::new();
        for (body_ref, body) in saved_bodies {
            anyhow::ensure!(
                body_ref.crate_ref == crate_ref,
                "saved body identity belongs to a different crate",
            );
            bodies_by_owner
                .entry(body.owner().declaration())
                .or_default()
                .push(*body_ref);
        }

        let mut worklist = UniqueVec::new();
        for root in roots {
            if saved_bodies
                .iter()
                .any(|(saved_body, _)| *saved_body == root)
            {
                worklist.push(root);
            }
        }

        let mut next = 0;
        while let Some(body_ref) = worklist.as_slice().get(next).copied() {
            next += 1;
            let items = saved_body_ir.all_items(DefMapRef::Body(body_ref))?;
            index.record_body_local_items(
                &items,
                file,
                &bodies_by_owner,
                &mut worklist,
                saved_body_ir,
            )?;
        }

        Ok(index)
    }

    fn record_body_local_items(
        &mut self,
        items: &[SemanticItemView<'_>],
        file: FileId,
        bodies_by_owner: &HashMap<DeclarationRef, ExpectedUnique<BodyRef>>,
        worklist: &mut UniqueVec<BodyRef>,
        bodies: &BodyReader<'_>,
    ) -> anyhow::Result<()> {
        for item in items {
            if item.source().file_id != file {
                continue;
            }
            let owner = match item.item() {
                SemanticItemRef::Function(function) => BodyOwner::Function(function),
                SemanticItemRef::Const(konst) => BodyOwner::Const(konst),
                SemanticItemRef::Static(static_ref) => BodyOwner::Static(static_ref),
                _ => continue,
            };
            if let Some(span) = item.location() {
                self.record_body_owner(
                    bodies
                        .declaration_location(
                            item.item().origin().origin_crate(),
                            item.source(),
                            span,
                        )?
                        .span,
                    owner,
                    bodies_by_owner,
                    worklist,
                );
            }
        }
        Ok(())
    }

    fn record_body_owner(
        &mut self,
        span: Span,
        owner: BodyOwner,
        bodies_by_owner: &HashMap<DeclarationRef, ExpectedUnique<BodyRef>>,
        worklist: &mut UniqueVec<BodyRef>,
    ) {
        let Some(body_ref) = bodies_by_owner
            .get(&owner.declaration())
            .and_then(ExpectedUnique::as_option)
            .copied()
        else {
            return;
        };
        self.bodies.entry(span).or_default().push(body_ref);
        worklist.push(body_ref);
    }

    pub(super) fn body_ref_at(&self, saved_span: Span) -> Option<BodyRef> {
        self.bodies
            .get(&saved_span)
            .and_then(ExpectedUnique::as_option)
            .copied()
    }
}
