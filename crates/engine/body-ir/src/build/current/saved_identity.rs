//! Saved body lookups needed while rebuilding bodies from editor text.
//!
//! There are two lookups because saved roots and saved nested bodies live in different stores.
//! A crate-level function, const, or static comes from Semantic IR. A function declared inside
//! another body comes from that body's local item store instead. Keeping both lookups here lets
//! the builder ask a simple question at each point: which saved owner or `BodyRef`, if any, is safe
//! to reuse for this current declaration?

use std::collections::HashMap;

use rg_ir_model::{BodyRef, ConstRef, CrateRef, DefMapRef, StaticRef};
use rg_parse::{FileId, Span};
use rg_semantic_ir::ItemStore;
use rg_std::{ExpectedUnique, UniqueVec};

use crate::BodyOwner;

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
    pub(super) fn new(items: Option<&ItemStore>, crate_ref: CrateRef, file: FileId) -> Self {
        let mut owners = HashMap::<Span, ExpectedUnique<BodyOwner>>::new();
        let Some(items) = items else {
            return Self { owners };
        };

        for (function_ref, data) in items.functions_with_refs() {
            if data.source.file_id == file {
                owners
                    .entry(data.span)
                    .or_default()
                    .push(BodyOwner::Function(function_ref));
            }
        }
        for (id, data) in items.consts().iter_with_ids() {
            if data.source.file_id == file {
                owners
                    .entry(data.span)
                    .or_default()
                    .push(BodyOwner::Const(ConstRef {
                        origin: DefMapRef::Crate(crate_ref),
                        id,
                    }));
            }
        }
        for (id, data) in items.statics().iter_with_ids() {
            if data.source.file_id == file {
                owners
                    .entry(data.span)
                    .or_default()
                    .push(BodyOwner::Static(StaticRef {
                        origin: DefMapRef::Crate(crate_ref),
                        id,
                    }));
            }
        }

        Self { owners }
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
        saved_body_ir: &crate::BodyIrReadTxn<'_>,
        crate_ref: CrateRef,
        file: FileId,
        saved_bodies: &[(BodyRef, crate::BodyView<'_>)],
        roots: impl IntoIterator<Item = BodyRef>,
    ) -> anyhow::Result<Self> {
        let mut index = Self {
            bodies: HashMap::new(),
        };

        // Stored Body IR is keyed by `BodyRef`, while body-local item stores point at their nested
        // bodies through declaration identities. This lookup connects the two before we start the
        // selected-root walk below.
        let mut bodies_by_owner =
            HashMap::<rg_ir_model::identity::DeclarationRef, ExpectedUnique<BodyRef>>::new();
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
            let Some(items) = saved_body_ir.body_local_items(body_ref)? else {
                continue;
            };
            index.record_body_local_items(
                items.item_store(),
                file,
                &bodies_by_owner,
                &mut worklist,
            );
        }

        Ok(index)
    }

    fn record_body_local_items(
        &mut self,
        items: &ItemStore,
        file: FileId,
        bodies_by_owner: &HashMap<rg_ir_model::identity::DeclarationRef, ExpectedUnique<BodyRef>>,
        worklist: &mut UniqueVec<BodyRef>,
    ) {
        for (function, data) in items.functions_with_refs() {
            if data.source.file_id == file {
                self.record_body_owner(
                    data.span,
                    BodyOwner::Function(function),
                    bodies_by_owner,
                    worklist,
                );
            }
        }
        for (id, data) in items.consts().iter_with_ids() {
            if data.source.file_id == file {
                self.record_body_owner(
                    data.span,
                    BodyOwner::Const(ConstRef {
                        origin: items.origin(),
                        id,
                    }),
                    bodies_by_owner,
                    worklist,
                );
            }
        }
        for (id, data) in items.statics().iter_with_ids() {
            if data.source.file_id == file {
                self.record_body_owner(
                    data.span,
                    BodyOwner::Static(StaticRef {
                        origin: items.origin(),
                        id,
                    }),
                    bodies_by_owner,
                    worklist,
                );
            }
        }
    }

    fn record_body_owner(
        &mut self,
        span: Span,
        owner: BodyOwner,
        bodies_by_owner: &HashMap<rg_ir_model::identity::DeclarationRef, ExpectedUnique<BodyRef>>,
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
