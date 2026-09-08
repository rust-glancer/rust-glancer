//! Declaration storage shared by lexical bodies and current declaration headers.
//!
//! Both sources have item payloads and declaration scopes. Expressions and inferred facts are
//! irrelevant to collection, so a header can use this pipeline without manufacturing a body.

mod def_map;
mod item_store;
mod lower;

pub(crate) use lower::LocalItemLowering;

use anyhow::Context as _;
use rg_def_map::DefMapSource;
use rg_ir_model::BodyRef;
use rg_package_store::PackageStoreError;

use crate::{BodyData, BodyLocalItems, BodySource, BodySourceItems, ScopeData};

use self::{def_map::LocalDefMapCollector, item_store::LocalItemStoreCollector};

/// Borrow the declaration part of a lowering result for the shared collectors.
///
/// Bodies supply their lexical scopes; an impl-header query supplies one scope holding the
/// impl. Collection uses the scopes' item lists and parents, without reading their bindings.
#[derive(Clone, Copy)]
pub(crate) struct LocalItemSource<'a> {
    pub(crate) source: BodySource,
    pub(crate) scopes: &'a [ScopeData],
    pub(crate) items: &'a BodySourceItems,
}

impl<'a> LocalItemSource<'a> {
    pub(crate) fn for_body(body: &'a BodyData) -> Self {
        Self {
            source: body.source(),
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
        cancellation: &rg_std::CancellationToken,
    ) -> anyhow::Result<BodyLocalItems>
    where
        S: DefMapSource<Error = PackageStoreError> + Copy,
    {
        let def_map = LocalDefMapCollector::new(origin, self)
            .collect(cancellation)
            .context("collect local names")?
            .finalize(surrounding, cancellation)
            .context("finalize local declarations")?;
        let items = LocalItemStoreCollector::new(self.items, &def_map)
            .collect(cancellation)
            .context("lower local signatures")?;
        rg_std::check_cancel!(cancellation, "finish local declarations");
        Ok(BodyLocalItems::new(def_map, items))
    }
}
