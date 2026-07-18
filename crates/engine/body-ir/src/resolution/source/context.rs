//! Shared provider construction for body resolution.
//!
//! Resolution components should not each remember how to wire DefMap, item-store, lookup-index,
//! solver-session, and body providers together. This context keeps that routing in one place while
//! still exposing only read-only access to the active body.

use rg_def_map::{DefMapQuery, DefMapSource};
use rg_ir_model::{BodyRef, Path, ScopeId, TypePathResolution};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemLookupIndex, ItemStoreQuery, ItemStoreSource};
use rg_ty::{
    Autoderef, ImplMatcher, ItemPathQuery, SemanticSignatureQuery, TraitSelectionQuery,
    TraitSelectionSession, TyContext, TypeLoweringAnchor, TypePathResolver,
};

use crate::{BodyData, BodyView, ir::BodyQueryView};

use crate::resolution::query::{
    BodyAssociatedItemQuery, BodyCallQuery, BodyFieldQuery, BodyFunctionQuery, BodyGenericsQuery,
    BodyLocalItemQuery, BodyMethodQuery, BodyTraitQuery, BodyTypeAliasQuery, BodyTypeContextQuery,
    BodyTypePathQuery, BodyValuePathQuery, TypeRefResolutionQuery,
};

use super::BodyQuerySource;

type BodySemanticSignatureQuery<'context, 'query, D, I> = SemanticSignatureQuery<
    'query,
    BodyQuerySource<'query, D, I>,
    BodyQuerySource<'query, D, I>,
    &'context BodyResolutionContext<'query, D, I>,
>;

type BodyImplMatcher<'context, 'query, D, I> = ImplMatcher<
    'query,
    BodyQuerySource<'query, D, I>,
    BodyQuerySource<'query, D, I>,
    &'context BodyResolutionContext<'query, D, I>,
>;

/// Read-only provider bundle shared by body semantic queries.
///
/// The context keeps DefMap, item-store, semantic-index, trait-selection, and active-body routing
/// coherent while small query objects own the actual operations. A finalized consumer supplies
/// `BodyView`; indexing can instead supply structural-only data or a crate-private inference
/// snapshot. Query APIs that need types therefore read through `query_body`, whose source is
/// explicit at construction time.
#[derive(Clone)]
pub struct BodyResolutionContext<'a, D, I> {
    source: BodyQuerySource<'a, D, I>,
    ty: TyContext<'a, BodyQuerySource<'a, D, I>, BodyQuerySource<'a, D, I>>,
}

impl<'a, D, I> BodyResolutionContext<'a, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Clone,
    I: ItemStoreSource<'a, Error = PackageStoreError> + Clone,
{
    pub fn new(
        def_maps: D,
        item_stores: I,
        body_ref: BodyRef,
        body: BodyView<'a>,
        semantic_index: &'a ItemLookupIndex,
        trait_selection: TraitSelectionSession,
    ) -> Self {
        Self::from_source(
            BodyQuerySource::new(def_maps, item_stores, body_ref, body),
            semantic_index,
            trait_selection,
        )
    }

    /// Build a context for structural queries before semantic facts exist.
    ///
    /// Only operations backed by `BodyData` are valid in this phase. Asking the resulting context
    /// for expression or binding facts is a programming error rather than an unknown result.
    pub(crate) fn for_structure(
        def_maps: D,
        item_stores: I,
        body_ref: BodyRef,
        body: &'a BodyData,
        semantic_index: &'a ItemLookupIndex,
        trait_selection: TraitSelectionSession,
    ) -> Self {
        Self::from_source(
            BodyQuerySource::for_structure(def_maps, item_stores, body_ref, body),
            semantic_index,
            trait_selection,
        )
    }

    /// Build a context over one finalized or inference-time semantic query view.
    pub(crate) fn for_query(
        def_maps: D,
        item_stores: I,
        body_ref: BodyRef,
        body: BodyQueryView<'a>,
        semantic_index: &'a ItemLookupIndex,
        trait_selection: TraitSelectionSession,
    ) -> Self {
        Self::from_source(
            BodyQuerySource::for_query(def_maps, item_stores, body_ref, body),
            semantic_index,
            trait_selection,
        )
    }

    fn from_source(
        source: BodyQuerySource<'a, D, I>,
        semantic_index: &'a ItemLookupIndex,
        trait_selection: TraitSelectionSession,
    ) -> Self {
        assert_eq!(
            source.body_ref().crate_ref,
            trait_selection.use_site(),
            "trait-selection session must match the body use-site crate"
        );
        let ty = TyContext::new(
            source.clone(),
            source.clone(),
            semantic_index,
            trait_selection,
        );
        Self { source, ty }
    }
}

impl<'a, D, I> BodyResolutionContext<'a, D, I> {
    pub(crate) fn body_ref(&self) -> BodyRef {
        self.source.body_ref()
    }

    pub(crate) fn body(&self) -> &'a BodyData {
        self.source.body()
    }

    pub(crate) fn query_body(&self) -> BodyQueryView<'a> {
        self.source.query_body()
    }

    pub(crate) fn semantic_index(&self) -> &'a ItemLookupIndex {
        self.ty.lookup_index()
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
        self.ty.item_paths().clone()
    }

    pub(crate) fn signatures<'context>(
        &'context self,
    ) -> BodySemanticSignatureQuery<'context, 'a, D, I> {
        let source = self.source;
        SemanticSignatureQuery::with_resolver(source, source, self)
    }

    pub fn type_path_query(&self) -> BodyTypePathQuery<'a, D, I> {
        BodyTypePathQuery::new(self.clone())
    }

    pub fn value_paths(&self) -> BodyValuePathQuery<'a, D, I> {
        BodyValuePathQuery::new(self.clone())
    }

    pub(crate) fn type_refs(&self, scope: ScopeId) -> TypeRefResolutionQuery<'a, D, I> {
        TypeRefResolutionQuery::new(self.clone(), scope)
    }

    pub(crate) fn type_contexts(&self) -> BodyTypeContextQuery<'a, D, I> {
        BodyTypeContextQuery::new(self.clone())
    }

    pub(crate) fn type_aliases(&self) -> BodyTypeAliasQuery<'a, D, I> {
        BodyTypeAliasQuery::new(self.clone())
    }

    pub(crate) fn generics(&self) -> BodyGenericsQuery<'a, D, I> {
        BodyGenericsQuery::new(self.clone())
    }

    pub(crate) fn associated_items(&self) -> BodyAssociatedItemQuery<'a, D, I> {
        BodyAssociatedItemQuery::new(self.clone())
    }

    pub(crate) fn traits(&self) -> BodyTraitQuery<'a, D, I> {
        BodyTraitQuery::new(self.clone())
    }

    pub(crate) fn calls(&self) -> BodyCallQuery<'a, D, I> {
        BodyCallQuery::new(self.clone())
    }

    pub(crate) fn fields(&self) -> BodyFieldQuery<'a, D, I> {
        BodyFieldQuery::new(self.clone())
    }

    pub(crate) fn functions(&self) -> BodyFunctionQuery<'a, D, I> {
        BodyFunctionQuery::new(self.clone())
    }

    pub(crate) fn body_local_items(&self) -> BodyLocalItemQuery<'a, D, I> {
        BodyLocalItemQuery::new(self.clone())
    }

    pub fn methods(&self) -> BodyMethodQuery<'a, D, I> {
        BodyMethodQuery::new(self.clone())
    }

    pub(crate) fn impl_matcher<'context>(&'context self) -> BodyImplMatcher<'context, 'a, D, I> {
        ImplMatcher::with_resolver(self.ty.clone(), self)
    }

    pub(crate) fn autoderef(
        &self,
    ) -> Autoderef<'a, BodyQuerySource<'a, D, I>, BodyQuerySource<'a, D, I>> {
        Autoderef::new(self.ty.clone())
    }

    /// Build trait selection in this body's crate-scoped solver session.
    pub(crate) fn trait_selection(
        &self,
    ) -> TraitSelectionQuery<'a, BodyQuerySource<'a, D, I>, BodyQuerySource<'a, D, I>> {
        TraitSelectionQuery::new(self.ty.clone())
    }
}

impl<'a, D, I> TypePathResolver for BodyResolutionContext<'a, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'a, Error = PackageStoreError> + Copy,
{
    type Error = PackageStoreError;

    fn resolve_type_path(
        &self,
        anchor: TypeLoweringAnchor,
        path: &Path,
    ) -> Result<TypePathResolution, Self::Error> {
        match anchor {
            TypeLoweringAnchor::Scope(scope) => {
                self.type_path_query().resolve_in_scope(scope, path)
            }
            TypeLoweringAnchor::Context(context) => {
                self.type_path_query().resolve_in_context(context, path)
            }
        }
    }
}
