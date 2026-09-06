//! Shared read handle for indexed-data views.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::Context as _;
use rg_body_ir::{BodyIrReadTxn, CurrentSourceBuilder, CurrentSourceStore};
use rg_def_map::DefMapReadTxn;
use rg_def_map::{DefMap, DefMapSource};
use rg_ir_model::{BodyRef, CrateRef, DefMapRef, ModuleRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::SemanticIrReadTxn;
use rg_semantic_ir::{
    CrateItemQuery, ItemLookupQuery, ItemLookupQueryCache, ItemStore, ItemStoreSource,
    TypePathContext, TypePathResolution,
};
use rg_std::{CancellationToken, UniqueVec};
use rg_text::RustEdition;
use rg_ty::{ItemPathQuery, TraitSelectionSession, TypeLoweringAnchor, TypePathResolver};

/// Read-only database handle used by all indexed-data views.
///
/// Saved readers and prepared current declarations stay frozen for the request. Trait-selection
/// sessions and item lookup caches hold derived query state: filling them never changes which
/// declarations or bodies are visible through this handle.
#[derive(Debug, Clone)]
pub struct IndexedViewDb<'db> {
    pub(crate) def_map: DefMapReadTxn<'db>,
    pub(crate) semantic_ir: SemanticIrReadTxn<'db>,
    pub(crate) body_ir: BodyIrReadTxn<'db>,
    trait_selection: Arc<Mutex<HashMap<CrateRef, TraitSelectionSession>>>,
    body_trait_selection: Arc<Mutex<HashMap<BodyRef, TraitSelectionSession>>>,
    item_lookup_cache: ItemLookupQueryCache,
    cancellation: CancellationToken,
}

impl rg_std::Cancelable for IndexedViewDb<'_> {
    fn check_cancelled(&self, checkpoint: &'static str) -> Result<(), rg_std::Cancelled> {
        rg_std::Cancelable::check_cancelled(&self.cancellation, checkpoint)
    }
}

impl<'db> IndexedViewDb<'db> {
    pub fn new(
        def_map: DefMapReadTxn<'db>,
        semantic_ir: SemanticIrReadTxn<'db>,
        body_ir: BodyIrReadTxn<'db>,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            body_ir,
            trait_selection: Arc::new(Mutex::new(HashMap::new())),
            body_trait_selection: Arc::new(Mutex::new(HashMap::new())),
            item_lookup_cache: ItemLookupQueryCache::new(),
            cancellation,
        }
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    /// Lend the saved readers and lookup cache used by the eventual request queries.
    pub fn current_source_builder<'request>(
        &'request self,
        source: &'request rg_parse::CurrentSource,
    ) -> CurrentSourceBuilder<'request, 'db> {
        CurrentSourceBuilder::new(
            &self.def_map,
            &self.semantic_ir,
            &self.body_ir,
            source,
            self.item_lookup_cache.clone(),
            self.cancellation.clone(),
        )
    }

    pub fn with_current_source(mut self, current: CurrentSourceStore) -> Self {
        self.body_ir = self.body_ir.with_current_source(current);
        self
    }

    /// Only the declaration contexts selected for ordinary source scanning are exposed here.
    /// Complete impls prepared alongside body contexts remain private to member queries.
    pub(crate) fn current_signature_origins(
        &self,
        crate_ref: CrateRef,
        file_id: rg_parse::FileId,
    ) -> impl Iterator<Item = DefMapRef> + '_ {
        self.body_ir.current_signature_origins(crate_ref, file_id)
    }

    /// Keep temporary generic and impl identities while looking up paths in their containing scope.
    pub(crate) fn current_signature_context(
        &self,
        mut context: TypePathContext,
    ) -> Result<TypePathContext, PackageStoreError> {
        context.module = self.body_ir.signature_lookup_module(context.module)?;
        Ok(context)
    }

    pub fn selected_current_impl(
        &self,
        crate_ref: CrateRef,
        file: rg_parse::FileId,
        span: rg_parse::Span,
    ) -> Option<rg_ir_model::ImplRef> {
        self.body_ir.selected_current_impl(crate_ref, file, span)
    }

    pub(crate) fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&ItemStore>, PackageStoreError> {
        ItemStoreSource::item_store_for_origin(&self, origin)
    }

    pub(crate) fn def_map_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&DefMap>, PackageStoreError> {
        match origin {
            DefMapRef::Crate(crate_ref) => self.def_map.def_map(crate_ref),
            DefMapRef::Body(body_ref) => self.body_ir.body_def_map(body_ref),
        }
    }

    /// Return whether a body identity belongs to request-local current Body IR.
    pub fn is_current_body(&self, body_ref: BodyRef) -> bool {
        self.body_ir.is_current_body(body_ref)
    }

    /// Return whether a semantic origin was created from this request's editor source.
    pub fn is_current_origin(&self, origin: DefMapRef) -> bool {
        self.body_ir.is_current_origin(origin)
    }

    /// Assemble the declarations visible from one crate without copying dependency indexes.
    ///
    /// Semantic IR owns one local index per crate. This request-local query keeps those indexes
    /// borrowed and adds only the small visibility and memoization layer needed by type queries.
    pub(crate) fn item_lookup_query(
        &self,
        use_site: CrateRef,
    ) -> anyhow::Result<ItemLookupQuery<'_>> {
        ItemLookupQuery::build_with_cache(
            &CrateItemQuery::new(&self.def_map, &self.semantic_ir, use_site),
            &self.item_lookup_cache,
            &self.cancellation,
        )
        .context("assemble visible semantic item indexes")
    }

    /// Return the solver session shared by queries at one crate use site.
    ///
    /// `IndexedViewDb` lives for one analysis request, so the potentially large Chalk program and
    /// candidate indexes are reused within that request and released with its frozen read
    /// transactions. Keeping session creation here also prevents individual view adapters from
    /// silently starting isolated solver state when a shared session is already available.
    pub fn trait_selection(&self, use_site: CrateRef) -> TraitSelectionSession {
        self.trait_selection
            .lock()
            .expect("trait-selection session map lock should not be poisoned")
            .entry(use_site)
            .or_insert_with(|| TraitSelectionSession::new(use_site))
            .clone()
            .with_cancellation(self.cancellation.clone())
    }

    /// Return the inference scope owned by one body in this analysis request.
    ///
    /// Separate bodies share the expensive crate-semantic solver state, but not answers that may
    /// contain local inference variables or synthetic body identities. Repeated queries for the
    /// same body reuse its scope until the request-owned view is dropped.
    pub(crate) fn trait_selection_for_body(&self, body_ref: BodyRef) -> TraitSelectionSession {
        let crate_session = self.trait_selection(body_ref.crate_ref);
        self.body_trait_selection
            .lock()
            .expect("body trait-selection session map lock should not be poisoned")
            .entry(body_ref)
            .or_insert_with(|| crate_session.for_body(body_ref))
            .clone()
            .with_cancellation(self.cancellation.clone())
    }

    /// Returns the edition whose syntax rules apply at a crate_ref use site.
    pub fn crate_edition(&self, crate_ref: CrateRef) -> Result<RustEdition, PackageStoreError> {
        self.def_map.package_edition(crate_ref.package)
    }

    /// Returns the edition whose syntax rules apply to declarations owned by this origin.
    pub fn origin_edition(&self, origin: DefMapRef) -> Result<RustEdition, PackageStoreError> {
        self.crate_edition(origin.origin_crate())
    }
}

/// Resolve request-local signature paths from their real containing module.
///
/// Generic and impl identities remain attached to the temporary declaration. Only the module
/// lookup starting point is replaced; after that, the ordinary indexed path query owns all name
/// resolution rules.
impl TypePathResolver for IndexedViewDb<'_> {
    type Error = PackageStoreError;

    fn resolve_type_path(
        &self,
        anchor: TypeLoweringAnchor,
        path: &rg_ir_model::Path,
    ) -> Result<TypePathResolution, Self::Error> {
        let TypeLoweringAnchor::Context(context) = anchor else {
            return Ok(TypePathResolution::Unknown);
        };
        let context = self.current_signature_context(context)?;
        ItemPathQuery::new(self, self).resolve_type_path(context, path)
    }
}

impl<'a, 'db> ItemStoreSource<'a> for &'a IndexedViewDb<'db> {
    type Error = PackageStoreError;

    fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&'a ItemStore>, PackageStoreError> {
        match origin {
            DefMapRef::Crate(crate_ref) => self.semantic_ir.items(crate_ref),
            DefMapRef::Body(body_ref) => self.body_ir.body_item_store(body_ref),
        }
    }

    fn included_stores(&self) -> Result<Vec<&'a ItemStore>, PackageStoreError> {
        self.semantic_ir.included_stores()
    }
}

impl DefMapSource for &IndexedViewDb<'_> {
    type Error = PackageStoreError;

    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, PackageStoreError> {
        match origin {
            DefMapRef::Crate(crate_ref) => self.def_map.def_map(crate_ref),
            DefMapRef::Body(body_ref) => self.body_ir.body_def_map(body_ref),
        }
    }

    fn crate_is_proc_macro(&self, crate_ref: CrateRef) -> Result<bool, PackageStoreError> {
        self.def_map.crate_is_proc_macro(crate_ref)
    }

    fn extern_root(
        &self,
        crate_ref: CrateRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.def_map.extern_root(crate_ref, name)
    }

    fn extern_roots(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Vec<(String, ModuleRef)>, PackageStoreError> {
        self.def_map.extern_roots(crate_ref)
    }

    fn prelude_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.def_map.prelude_module(crate_ref)
    }

    fn item_lookup_dependencies(
        &self,
        crate_ref: CrateRef,
    ) -> Result<UniqueVec<CrateRef>, PackageStoreError> {
        self.def_map.item_lookup_dependencies(crate_ref)
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.def_map.root_module(crate_ref)
    }
}
