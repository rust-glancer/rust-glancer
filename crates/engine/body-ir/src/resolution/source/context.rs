//! Shared provider construction for body resolution.
//!
//! Resolution components should not each remember how to wire DefMap, item-store, target, and body
//! lookup providers together. This context keeps that routing in one place while still exposing
//! only read-only access to the active body.

use rg_ir_model::BodyRef;
use rg_ir_storage::{
    DefMapQuery, DefMapSource, ItemLookupIndex, ItemStoreQuery, ItemStoreSource, TargetItemQuery,
};
use rg_package_store::PackageStoreError;
use rg_ty::{Autoderef, ImplMatcher, ItemPathQuery, IterationItemResolver};

use crate::ir::body::ResolvedBodyData;

use crate::resolution::query::{BodyLocalItemQuery, BodyReceiverFunctionQuery, BodyTypePathQuery};

use super::BodyQuerySource;

#[derive(Clone, Copy)]
pub(crate) struct BodyResolutionContext<'a, D, I> {
    source: BodyQuerySource<'a, D, I>,
    semantic_index: Option<&'a ItemLookupIndex>,
}

impl<'a, D, I> BodyResolutionContext<'a, D, I> {
    pub(crate) fn new(
        def_maps: D,
        item_stores: I,
        body_ref: BodyRef,
        body: &'a ResolvedBodyData,
        semantic_index: Option<&'a ItemLookupIndex>,
    ) -> Self {
        Self {
            source: BodyQuerySource::new(def_maps, item_stores, body_ref, body),
            semantic_index,
        }
    }

    pub(crate) fn body_ref(&self) -> BodyRef {
        self.source.body_ref()
    }

    pub(crate) fn body(&self) -> &'a ResolvedBodyData {
        self.source.body()
    }

    pub(crate) fn semantic_index(&self) -> Option<&'a ItemLookupIndex> {
        self.semantic_index
    }
}

impl<'a, D, I> BodyResolutionContext<'a, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'a, Error = PackageStoreError> + Copy,
{
    pub(crate) fn query_source(&self) -> BodyQuerySource<'a, D, I> {
        self.source
    }

    pub(crate) fn def_map_query(&self) -> DefMapQuery<BodyQuerySource<'a, D, I>> {
        DefMapQuery::new(self.source)
    }

    pub(crate) fn item_query(&self) -> ItemStoreQuery<'a, BodyQuerySource<'a, D, I>> {
        ItemStoreQuery::new(self.source)
    }

    pub(crate) fn item_paths(
        &self,
    ) -> ItemPathQuery<'a, BodyQuerySource<'a, D, I>, BodyQuerySource<'a, D, I>> {
        let source = self.source;
        ItemPathQuery::new(source, source)
    }

    pub(crate) fn target_items(
        &self,
    ) -> TargetItemQuery<'a, BodyQuerySource<'a, D, I>, BodyQuerySource<'a, D, I>> {
        let source = self.source;
        TargetItemQuery::new(source, source, self.source.body_ref().target)
    }

    pub(crate) fn type_path_query(&self) -> BodyTypePathQuery<'a, D, I> {
        BodyTypePathQuery::new(*self)
    }

    pub(crate) fn body_local_items(&self) -> BodyLocalItemQuery<'a, D, I> {
        BodyLocalItemQuery::new(*self)
    }

    pub(crate) fn receiver_functions(&self) -> BodyReceiverFunctionQuery<'a, D, I> {
        BodyReceiverFunctionQuery::new(*self)
    }

    pub(crate) fn impl_matcher(
        &self,
    ) -> ImplMatcher<'a, BodyQuerySource<'a, D, I>, BodyQuerySource<'a, D, I>> {
        ImplMatcher::new(self.item_paths(), self.target_items())
    }

    pub(crate) fn autoderef(
        &self,
    ) -> Autoderef<'a, BodyQuerySource<'a, D, I>, BodyQuerySource<'a, D, I>> {
        match self.semantic_index {
            Some(index) => Autoderef::with_index(self.item_paths(), self.target_items(), index),
            None => Autoderef::new(self.item_paths(), self.target_items()),
        }
    }

    pub(crate) fn iteration_items(
        &self,
    ) -> IterationItemResolver<'a, BodyQuerySource<'a, D, I>, BodyQuerySource<'a, D, I>> {
        match self.semantic_index {
            Some(index) => {
                IterationItemResolver::with_index(self.item_paths(), self.target_items(), index)
            }
            None => IterationItemResolver::new(self.item_paths(), self.target_items()),
        }
    }
}
