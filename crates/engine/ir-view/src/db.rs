//! Shared read handle for indexed-data views.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::Context as _;
use rg_body_ir::{
    BodyIrReadTxn, BodyLocalItems, CurrentBodyBuildCheckpoint, CurrentBodyBuildOutcome,
    CurrentBodyBuilder, CurrentBodySelection, CurrentBodySet,
};
use rg_def_map::DefMapReadTxn;
use rg_def_map::{DefMap, DefMapSource};
use rg_ir_model::{BodyRef, CrateRef, DefMapRef, ModuleRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::SemanticIrReadTxn;
use rg_semantic_ir::{
    CrateItemQuery, ItemLookupQuery, ItemLookupQueryCache, ItemStore, ItemStoreSource,
    TypePathContext, TypePathResolution,
};
use rg_std::UniqueVec;
use rg_text::RustEdition;
use rg_ty::{ItemPathQuery, TraitSelectionSession, TypeLoweringAnchor, TypePathResolver};

/// Read-only database handle used by all indexed-data views.
///
/// The handle deliberately contains the concrete frozen storage transactions. That keeps views
/// easy to extract as one crate first; a trait facade can replace these fields later once the
/// method surface settles. Trait-selection sessions are derived query state: they may fill solver
/// caches, but they never mutate the frozen project data exposed by this handle.
#[derive(Debug, Clone)]
pub struct IndexedViewDb<'db> {
    pub(crate) def_map: DefMapReadTxn<'db>,
    pub(crate) semantic_ir: SemanticIrReadTxn<'db>,
    pub(crate) body_ir: BodyIrReadTxn<'db>,
    trait_selection: Arc<Mutex<HashMap<CrateRef, TraitSelectionSession>>>,
    body_trait_selection: Arc<Mutex<HashMap<BodyRef, TraitSelectionSession>>>,
    item_lookup_cache: ItemLookupQueryCache,
    /// Small semantic stores created by a view query from the request's source.
    ///
    /// They are deliberately absent from `included_stores`: an unsaved declaration may explain
    /// the source currently under the cursor, but it must not become globally discoverable by
    /// unrelated lookup in the same request.
    request_local_items: Vec<Arc<BodyLocalItems>>,
    /// Saved module in whose scope each header-only request module was written.
    ///
    /// The temporary DefMap gives an edited impl, its generics, and its members semantic
    /// identities. Its synthetic module is only storage, however: paths such as `Service<Local>`
    /// must still start lookup in the real module containing the current syntax.
    request_local_module_fallbacks: HashMap<ModuleRef, ModuleRef>,
}

impl<'db> IndexedViewDb<'db> {
    pub fn new(
        def_map: DefMapReadTxn<'db>,
        semantic_ir: SemanticIrReadTxn<'db>,
        body_ir: BodyIrReadTxn<'db>,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            body_ir,
            trait_selection: Arc::new(Mutex::new(HashMap::new())),
            body_trait_selection: Arc::new(Mutex::new(HashMap::new())),
            item_lookup_cache: ItemLookupQueryCache::new(),
            request_local_items: Vec::new(),
            request_local_module_fallbacks: HashMap::new(),
        }
    }

    /// Build the current bodies chosen by one cursor or range selection.
    ///
    /// Selection decides which syntax roots start the worklist. Lowering may add nested bodies, so
    /// even a cursor selection returns a collection rather than one body.
    #[allow(clippy::too_many_arguments)]
    pub fn build_current_bodies(
        &self,
        parse_package: &rg_parse::Package,
        crate_ref: CrateRef,
        file: rg_parse::FileId,
        current_source: &rg_parse::CurrentSource,
        associations: &rg_parse::DeclarationAssociationIndex,
        selection: CurrentBodySelection,
        synthetic_body_ref: impl FnMut() -> anyhow::Result<rg_ir_model::BodyRef>,
        checkpoint: impl FnMut(CurrentBodyBuildCheckpoint) -> anyhow::Result<()>,
    ) -> anyhow::Result<CurrentBodyBuildOutcome> {
        CurrentBodyBuilder::new(
            parse_package,
            &self.def_map,
            &self.semantic_ir,
            &self.body_ir,
            crate_ref,
            file,
            current_source,
            associations,
            self.item_lookup_cache.clone(),
            selection,
        )
        .with_trait_selection(self.trait_selection(crate_ref))
        .build(synthetic_body_ref, checkpoint)
    }

    /// Allocate the first request-only body id after this crate's saved body ids.
    pub fn first_synthetic_body_ref(
        &self,
        crate_ref: CrateRef,
    ) -> Result<rg_ir_model::BodyRef, PackageStoreError> {
        self.body_ir.first_synthetic_body_ref(crate_ref)
    }

    /// Allocate an identity that cannot overlap saved or rebuilt bodies in this request.
    pub(crate) fn next_synthetic_body_ref(
        &self,
        crate_ref: CrateRef,
    ) -> Result<rg_ir_model::BodyRef, PackageStoreError> {
        let mut next = self.body_ir.next_synthetic_body_ref(crate_ref)?;
        for items in &self.request_local_items {
            let DefMapRef::Body(body_ref) = items.def_map().own_ref() else {
                continue;
            };
            if body_ref.crate_ref == crate_ref {
                next.body.0 = next.body.0.max(body_ref.body.0.saturating_add(1));
            }
        }
        Ok(next)
    }

    /// Add request-local body replacements without changing the saved transactions.
    pub fn with_current_body_set(mut self, current: CurrentBodySet) -> Self {
        self.body_ir = self.body_ir.with_current_body_set(current);
        self
    }

    /// Layer one source-derived item context over this cloned request view.
    ///
    /// The overlay is useful when a query needs semantic lowering for one declaration that has no
    /// saved identity yet. It remains visible only through the returned view and is dropped with
    /// that view.
    pub(crate) fn with_request_local_items(
        mut self,
        items: BodyLocalItems,
        fallback_module: ModuleRef,
    ) -> Self {
        self.request_local_module_fallbacks.extend(
            items
                .def_map()
                .module_refs()
                .map(|module| (module, fallback_module)),
        );
        self.request_local_items.push(Arc::new(items));
        self
    }

    /// Return request-local declaration stores that may describe this source file.
    ///
    /// Rebuilt roots keep their declarations in Body IR. Header-only views use the explicit
    /// overlay above. Neither collection participates in crate-wide discovery, so callers must
    /// opt into this iterator when interpreting the current declaration under the cursor.
    ///
    /// ```text
    /// fn load<Current>(_: Cur$0) {} -> item origin stored beside the rebuilt body
    /// impl Service for Wor$0 {}      -> temporary item origin with no body
    /// ```
    pub(crate) fn current_signature_origins(
        &self,
        crate_ref: CrateRef,
        file_id: rg_parse::FileId,
    ) -> Result<Vec<DefMapRef>, PackageStoreError> {
        let mut origins = Vec::new();
        for body_ref in self.body_ir.current_body_refs(crate_ref, file_id) {
            if self.body_ir.body_item_store(body_ref)?.is_some() {
                origins.push(DefMapRef::Body(body_ref));
            }
        }
        for origin in self
            .request_local_items
            .iter()
            .map(|items| items.def_map().own_ref())
            .filter(|origin| origin.origin_crate() == crate_ref)
        {
            if !origins.contains(&origin) {
                origins.push(origin);
            }
        }
        Ok(origins)
    }

    /// Replace a temporary declaration module with the saved module that supplies its names.
    ///
    /// A rebuilt body already records this fallback on `BodyData`. A header-only overlay has no
    /// body, so its mapping is retained explicitly by this request database.
    ///
    /// For `impl<Local> Service for model::Worker<Local>`, this keeps the temporary impl identity
    /// but starts lookup for `Service` and `model` in the saved containing module.
    pub(crate) fn current_signature_context(
        &self,
        mut context: TypePathContext,
    ) -> Result<TypePathContext, PackageStoreError> {
        if let Some(fallback) = self.request_local_module_fallbacks.get(&context.module) {
            context.module = *fallback;
            return Ok(context);
        }
        let DefMapRef::Body(body_ref) = context.module.origin else {
            return Ok(context);
        };
        let Some(body) = self.body_ir.body(body_ref)? else {
            return Ok(context);
        };
        if context.module == body.owner_module() {
            context.module = body.fallback_module();
        }
        Ok(context)
    }

    fn request_local_items(&self, origin: DefMapRef) -> Option<&BodyLocalItems> {
        self.request_local_items
            .iter()
            .rev()
            .find(|items| items.def_map().own_ref() == origin)
            .map(Arc::as_ref)
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
        if let Some(items) = self.request_local_items(origin) {
            return Ok(Some(items.def_map()));
        }
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
        match origin {
            DefMapRef::Crate(_) => false,
            DefMapRef::Body(body_ref) => {
                self.body_ir.is_current_body(body_ref) || self.request_local_items(origin).is_some()
            }
        }
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
        )
        .context("assemble visible semantic item indexes")
    }

    /// Return the solver session shared by queries at one crate use site.
    ///
    /// `IndexedViewDb` lives for one analysis request, so the potentially large Chalk program and
    /// candidate indexes are reused within that request and released with its frozen read
    /// transactions. Keeping session creation here also prevents individual view adapters from
    /// silently starting isolated solver state when a shared session is already available.
    pub(crate) fn trait_selection(&self, use_site: CrateRef) -> TraitSelectionSession {
        self.trait_selection
            .lock()
            .expect("trait-selection session map lock should not be poisoned")
            .entry(use_site)
            .or_insert_with(|| TraitSelectionSession::new(use_site))
            .clone()
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
        if let Some(items) = self.request_local_items(origin) {
            return Ok(Some(items.item_store()));
        }
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
        if let Some(items) = self.request_local_items(origin) {
            return Ok(Some(items.def_map()));
        }
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
