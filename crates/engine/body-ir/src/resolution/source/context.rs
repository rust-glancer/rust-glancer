//! Shared provider construction for body resolution.
//!
//! Resolution components should not each remember how to wire DefMap, item-store, crate, and body
//! lookup providers together. This context keeps that routing in one place while still exposing
//! only read-only access to the active body.

use rg_def_map::{DefMapQuery, DefMapSource};
use rg_ir_model::BodyRef;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{CrateItemQuery, ItemLookupIndex, ItemStoreQuery, ItemStoreSource};
use rg_ty::{
    Autoderef, ImplMatcher, ItemPathQuery, IterationItemResolver, SemanticSignatureQuery,
    TraitSelectionCache,
};

use crate::ir::body::ResolvedBodyData;

use crate::resolution::query::{
    BodyAssociatedItemQuery, BodyCallQuery, BodyFieldQuery, BodyFunctionQuery, BodyGenericsQuery,
    BodyLocalItemQuery, BodyMethodQuery, BodyTraitQuery, BodyTypeAliasQuery, BodyTypeContextQuery,
    BodyTypePathQuery, BodyTypePathResolver, BodyValuePathQuery, TypeRefResolutionQuery,
    TypeRefUseSite,
};

use super::BodyQuerySource;

#[derive(Clone, Copy)]
pub struct BodyResolutionContext<'a, D, I> {
    source: BodyQuerySource<'a, D, I>,
    semantic_index: &'a ItemLookupIndex,
    trait_selection_cache: Option<&'a TraitSelectionCache>,
}

impl<'a, D, I> BodyResolutionContext<'a, D, I> {
    pub fn new(
        def_maps: D,
        item_stores: I,
        body_ref: BodyRef,
        body: &'a ResolvedBodyData,
        semantic_index: &'a ItemLookupIndex,
    ) -> Self {
        Self {
            source: BodyQuerySource::new(def_maps, item_stores, body_ref, body),
            semantic_index,
            trait_selection_cache: None,
        }
    }

    pub(crate) fn with_trait_selection_cache(
        def_maps: D,
        item_stores: I,
        body_ref: BodyRef,
        body: &'a ResolvedBodyData,
        semantic_index: &'a ItemLookupIndex,
        trait_selection_cache: &'a TraitSelectionCache,
    ) -> Self {
        Self {
            source: BodyQuerySource::new(def_maps, item_stores, body_ref, body),
            semantic_index,
            trait_selection_cache: Some(trait_selection_cache),
        }
    }

    pub(crate) fn body_ref(&self) -> BodyRef {
        self.source.body_ref()
    }

    pub(crate) fn body(&self) -> &'a ResolvedBodyData {
        self.source.body()
    }

    pub(crate) fn semantic_index(&self) -> &'a ItemLookupIndex {
        self.semantic_index
    }

    pub(crate) fn trait_selection_cache(&self) -> TraitSelectionCache {
        self.trait_selection_cache.cloned().unwrap_or_default()
    }
}

impl<'a, D, I> BodyResolutionContext<'a, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'a, Error = PackageStoreError> + Copy,
{
    pub(crate) fn def_map_query(&self) -> DefMapQuery<BodyQuerySource<'a, D, I>> {
        DefMapQuery::new(self.source)
    }

    pub(crate) fn def_map_source(&self) -> BodyQuerySource<'a, D, I> {
        self.source
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

    pub(crate) fn signatures(
        &self,
    ) -> SemanticSignatureQuery<
        'a,
        BodyQuerySource<'a, D, I>,
        BodyQuerySource<'a, D, I>,
        BodyTypePathResolver<'a, D, I>,
    > {
        let source = self.source;
        SemanticSignatureQuery::with_resolver(source, source, BodyTypePathResolver::new(*self))
    }

    pub(crate) fn crate_items(
        &self,
    ) -> CrateItemQuery<'a, BodyQuerySource<'a, D, I>, BodyQuerySource<'a, D, I>> {
        let source = self.source;
        CrateItemQuery::new(source, source, self.source.body_ref().crate_ref)
    }

    pub fn type_path_query(&self) -> BodyTypePathQuery<'a, D, I> {
        BodyTypePathQuery::new(*self)
    }

    pub fn value_paths(&self) -> BodyValuePathQuery<'a, D, I> {
        BodyValuePathQuery::new(*self)
    }

    pub(crate) fn type_refs(&self, use_site: TypeRefUseSite) -> TypeRefResolutionQuery<'a, D, I> {
        TypeRefResolutionQuery::new(*self, use_site)
    }

    pub(crate) fn type_contexts(&self) -> BodyTypeContextQuery<'a, D, I> {
        BodyTypeContextQuery::new(*self)
    }

    pub(crate) fn type_aliases(&self) -> BodyTypeAliasQuery<'a, D, I> {
        BodyTypeAliasQuery::new(*self)
    }

    pub(crate) fn generics(&self) -> BodyGenericsQuery<'a, D, I> {
        BodyGenericsQuery::new(*self)
    }

    pub(crate) fn associated_items(&self) -> BodyAssociatedItemQuery<'a, D, I> {
        BodyAssociatedItemQuery::new(*self)
    }

    pub(crate) fn traits(&self) -> BodyTraitQuery<'a, D, I> {
        BodyTraitQuery::new(*self)
    }

    pub(crate) fn calls(&self) -> BodyCallQuery<'a, D, I> {
        BodyCallQuery::new(*self)
    }

    pub(crate) fn fields(&self) -> BodyFieldQuery<'a, D, I> {
        BodyFieldQuery::new(*self)
    }

    pub(crate) fn functions(&self) -> BodyFunctionQuery<'a, D, I> {
        BodyFunctionQuery::new(*self)
    }

    pub(crate) fn body_local_items(&self) -> BodyLocalItemQuery<'a, D, I> {
        BodyLocalItemQuery::new(*self)
    }

    pub fn methods(&self) -> BodyMethodQuery<'a, D, I> {
        BodyMethodQuery::new(*self)
    }

    pub(crate) fn impl_matcher(
        &self,
    ) -> ImplMatcher<
        'a,
        BodyQuerySource<'a, D, I>,
        BodyQuerySource<'a, D, I>,
        BodyTypePathResolver<'a, D, I>,
    > {
        ImplMatcher::with_resolver(
            self.item_paths(),
            self.crate_items(),
            BodyTypePathResolver::new(*self),
        )
        .with_cache(self.trait_selection_cache())
    }

    pub(crate) fn autoderef(
        &self,
    ) -> Autoderef<'a, BodyQuerySource<'a, D, I>, BodyQuerySource<'a, D, I>> {
        Autoderef::with_index(self.item_paths(), self.crate_items(), self.semantic_index)
            .with_cache(self.trait_selection_cache())
    }

    pub(crate) fn iteration_items(
        &self,
    ) -> IterationItemResolver<'a, BodyQuerySource<'a, D, I>, BodyQuerySource<'a, D, I>> {
        IterationItemResolver::with_index(
            self.item_paths(),
            self.crate_items(),
            self.semantic_index,
        )
        .with_cache(self.trait_selection_cache())
    }
}
