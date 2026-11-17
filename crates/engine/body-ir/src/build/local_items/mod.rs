//! Declaration storage for items nested within lexical bodies.
//!
//! Collect local names before lowering signatures, so those signatures can refer to other items
//! in the same body. Expressions and inferred facts are not needed for this step.

mod def_map;
mod item_store;
mod lower;

use anyhow::Context as _;
use rg_def_map::DefMapSource;
use rg_ir_model::BodyRef;
use rg_std::CancellationToken;

pub(crate) use self::lower::LocalItemLowering;
use self::{def_map::LocalDefMapCollector, item_store::LocalItemStoreCollector};
use crate::{BodyData, BodyLocalItems, BodySource, BodySourceItems, BodySourceStore, ScopeData};

/// Borrow the declaration part of a lowered body for name and signature collection.
///
/// Collection uses lexical scopes' item lists and parents, without reading local variable bindings.
#[derive(Clone, Copy)]
pub(crate) struct LocalItemSource<'a> {
    pub(crate) source: BodySource,
    pub(crate) scopes: &'a [ScopeData],
    pub(crate) items: &'a BodySourceItems,
}

impl<'a> LocalItemSource<'a> {
    pub(crate) fn for_body(body: &'a BodyData, sources: &'a BodySourceStore) -> Self {
        Self {
            source: sources.source(body.source()),
            scopes: body.scopes(),
            items: body.source_items(),
        }
    }

    /// Finalize the local names before lowering signatures that refer to those declarations.
    ///
    /// `surrounding` supplies definitions outside this origin, including already collected
    /// enclosing bodies when a nested body imports their local items.
    #[rg_std::cancelable("collect local declarations", token = cancellation)]
    pub(crate) fn collect<S>(
        self,
        origin: BodyRef,
        surrounding: S,
        cancellation: &CancellationToken,
    ) -> anyhow::Result<BodyLocalItems>
    where
        S: DefMapSource + Copy,
    {
        let def_map = LocalDefMapCollector::new(origin, self)
            .collect(cancellation)
            .context("collect local names")?
            .finalize(surrounding, cancellation)
            .context("finalize local declarations")?;
        let items = LocalItemStoreCollector::new(self.items, def_map)
            .collect(cancellation)
            .context("lower local signatures")?;
        rg_std::check_cancel!(cancellation, "finish local declarations");
        Ok(items)
    }
}
