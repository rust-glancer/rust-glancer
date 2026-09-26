//! Shared provider construction for body resolution.
//!
//! Resolution components should not each remember how to wire DefMap, item-store, lookup-query,
//! cancellation, and body providers together. This context keeps that routing in one place while
//! still exposing only read-only access to the active body.

use rg_def_map::{DefMapQuery, DefMapSource};
use rg_ir_model::{BodyRef, Path, ScopeId};
use rg_item_tree::TypeRef;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemLookupQuery, ItemStoreQuery, ItemStoreSource, TypePathResolution};
use rg_std::CancellationToken;
use rg_ty::{
    Ty, TyContext,
    lookup::{AssociatedItemCandidateRef, ImplQuery, ItemPathQuery},
    lowering::{TypeLoweringAnchor, TypePathResolver},
    signature::SemanticSignatureQuery,
};

use super::cache::{BodyLocalItemCache, BodyResolutionCaches, BodyTraitLookupCache};
use crate::{
    BodyData,
    resolution::query::{
        BodyAssociatedItemQuery, BodyFieldQuery, BodyFunctionQuery, BodyGenericsQuery,
        BodyImplQuery, BodyLocalItemQuery, BodyMethodQuery, BodyTraitQuery, BodyTypeContextQuery,
        BodyTypePathQuery, BodyValuePathQuery, TypeRefResolutionQuery,
    },
};

type BodySemanticSignatureQuery<'context, 'query, D, I> =
    SemanticSignatureQuery<'query, D, I, &'context BodyResolutionContext<'query, D, I>>;

/// Read-only provider bundle shared by body semantic queries.
///
/// The context keeps DefMap, item-store, item-lookup-query, and active-body routing
/// coherent while small query objects own the actual operations. Queries borrow immutable body
/// structure and receive any needed resolutions or live types explicitly. The same context can
/// therefore stay in place while inference updates its facts and type slots.
#[derive(Clone)]
pub struct BodyResolutionContext<'a, D, I> {
    def_maps: D,
    item_stores: I,
    body_ref: BodyRef,
    body: &'a BodyData,
    ty: TyContext<'a, D, I>,
    caches: BodyResolutionCaches,
}

impl<D, I> rg_std::Cancelable for BodyResolutionContext<'_, D, I> {
    fn check_cancelled(&self, checkpoint: &'static str) -> Result<(), rg_std::Cancelled> {
        rg_std::Cancelable::check_cancelled(&self.ty, checkpoint)
    }
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
        body: &'a BodyData,
        item_lookup_query: &ItemLookupQuery<'a>,
        cancellation: CancellationToken,
    ) -> Self {
        let ty = TyContext::new(
            def_maps.clone(),
            item_stores.clone(),
            item_lookup_query.clone(),
            body_ref.crate_ref,
            cancellation,
        );
        Self {
            def_maps,
            item_stores,
            body_ref,
            body,
            ty,
            caches: BodyResolutionCaches::default(),
        }
    }
}

impl<'a, D, I> BodyResolutionContext<'a, D, I> {
    pub(crate) fn body_ref(&self) -> BodyRef {
        self.body_ref
    }

    pub(crate) fn body(&self) -> &'a BodyData {
        self.body
    }

    pub(crate) fn item_lookup_query(&self) -> &ItemLookupQuery<'a> {
        self.ty.item_lookup()
    }

    pub(crate) fn trait_cache(&self) -> &BodyTraitLookupCache {
        &self.caches.traits
    }

    pub(crate) fn body_local_item_cache(&self) -> &BodyLocalItemCache {
        &self.caches.body_local_items
    }

    pub(crate) fn ty_context(&self) -> TyContext<'a, D, I>
    where
        D: Clone,
        I: Clone,
    {
        self.ty.clone()
    }
}

impl<'a, D, I> BodyResolutionContext<'a, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'a, Error = PackageStoreError> + Copy,
{
    pub(crate) fn def_map_query(&self) -> DefMapQuery<D> {
        DefMapQuery::new(self.def_maps)
    }

    pub(crate) fn def_map_source(&self) -> D {
        self.def_maps
    }

    pub(crate) fn item_query(&self) -> ItemStoreQuery<'a, I> {
        ItemStoreQuery::new(self.item_stores)
    }

    pub(crate) fn item_paths(&self) -> ItemPathQuery<'a, D, I> {
        self.ty.item_paths().clone()
    }

    pub(crate) fn live(&self) -> super::query::LiveBodyQuery<'a, D, I> {
        super::query::LiveBodyQuery::new(self.clone())
    }

    pub(crate) fn signatures<'context>(
        &'context self,
    ) -> BodySemanticSignatureQuery<'context, 'a, D, I> {
        SemanticSignatureQuery::with_resolver(self.def_maps, self.item_stores, self)
    }

    pub fn type_path_query(&self) -> BodyTypePathQuery<'a, D, I> {
        BodyTypePathQuery::new(self.clone())
    }

    /// Lower source-shaped type syntax with the generic and lexical scope of this body.
    pub fn resolve_type_ref(
        &self,
        scope: ScopeId,
        type_ref: &TypeRef,
    ) -> Result<Ty, PackageStoreError> {
        self.type_refs(scope).resolve(type_ref)
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

    pub(crate) fn generics(&self) -> BodyGenericsQuery<'a, D, I> {
        BodyGenericsQuery::new(self.clone())
    }

    pub(crate) fn associated_items(&self) -> BodyAssociatedItemQuery<'a, D, I> {
        BodyAssociatedItemQuery::new(self.clone())
    }

    /// Return every associated declaration that may follow a rich body path prefix.
    ///
    /// This is the narrow public adapter used by editor views. The body query remains private so
    /// callers cannot accidentally bypass body-local item overlays or owner-scoped type lowering.
    pub fn associated_item_candidates(
        &self,
        scope: ScopeId,
        prefix: &crate::BodyAssociatedPathPrefix,
    ) -> Result<Vec<AssociatedItemCandidateRef>, PackageStoreError> {
        self.associated_items().candidates_for_prefix(scope, prefix)
    }

    /// Return declarations from the explicitly named trait and its supertraits.
    pub fn trait_associated_item_candidates(
        &self,
        scope: ScopeId,
        trait_ref: &TypeRef,
    ) -> Result<Vec<AssociatedItemCandidateRef>, PackageStoreError> {
        self.associated_items()
            .candidates_for_trait_ref(scope, trait_ref)
    }

    pub(crate) fn traits(&self) -> BodyTraitQuery<'a, D, I> {
        BodyTraitQuery::new(self.clone())
    }

    pub fn fields(&self) -> BodyFieldQuery<'a, D, I> {
        BodyFieldQuery::new(self.clone())
    }

    pub(crate) fn functions(&self) -> BodyFunctionQuery<'a, D, I> {
        BodyFunctionQuery::new(self.clone())
    }

    pub(crate) fn body_local_items(&self) -> BodyLocalItemQuery<'_, 'a, D, I> {
        BodyLocalItemQuery::new(self)
    }

    pub(crate) fn impls(&self) -> BodyImplQuery<'_, 'a, D, I> {
        BodyImplQuery::new(self)
    }

    pub fn methods(&self) -> BodyMethodQuery<'a, D, I> {
        BodyMethodQuery::new(self.clone())
    }

    pub(crate) fn impl_query(&self) -> ImplQuery<'a, D, I, &Self> {
        ImplQuery::with_resolver(self.ty.clone(), self)
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

impl<'a, D, I> rg_ty::solver::SolverScope for BodyResolutionContext<'a, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'a, Error = PackageStoreError> + Copy,
{
    fn local_trait_impls(
        &self,
        trait_ref: rg_ir_model::TraitDefRef,
    ) -> Result<Vec<rg_ir_model::TraitImplRef>, PackageStoreError> {
        Ok(self
            .body_local_items()
            .trait_impls_for_traits(&[trait_ref])?
            .collect())
    }

    fn generic_owner(&self) -> Option<rg_ir_model::GenericDefRef> {
        Some(self.body().owner().generic_def())
    }

    fn declaration_cache(&self) -> Option<&rg_ty::solver::DeclarationCache> {
        Some(&self.caches.solver_declarations)
    }
}
