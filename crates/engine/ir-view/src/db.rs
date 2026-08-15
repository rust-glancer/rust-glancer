//! Shared read handle for indexed-data views.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::Context as _;
use rg_body_ir::{
    BodyIrReadTxn, CurrentBodyBuildCheckpoint, CurrentBodyBuildOutcome, CurrentBodyBuilder,
    CurrentBodySelection, CurrentBodySet,
};
use rg_def_map::DefMapReadTxn;
use rg_def_map::{DefMap, DefMapSource};
use rg_ir_model::{BodyRef, CrateRef, DefMapRef, ModuleRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::SemanticIrReadTxn;
use rg_semantic_ir::{CrateItemQuery, ItemLookupIndex, ItemStore, ItemStoreSource};
use rg_text::RustEdition;
use rg_ty::TraitSelectionSession;

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
        supplemental_item_lookup_index: Option<&ItemLookupIndex>,
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
            supplemental_item_lookup_index,
            selection,
        )
        .with_trait_selection(self.trait_selection(crate_ref))
        .build(synthetic_body_ref, checkpoint)
    }

    /// Build the crate-wide lookup needed to resolve a current body before saved bodies are ready.
    ///
    /// An early-start project publishes DefMap and Semantic IR before it builds saved Body IR.
    /// Current bodies can use those already available declarations to build this smaller index
    /// without materializing every saved body. The value belongs to the request and crate, so the
    /// project snapshot prepares it once and shares it across current-body builds in that crate.
    /// If saved Body IR already published the index, this method returns `None` instead.
    pub fn build_supplemental_current_body_item_lookup_index(
        &self,
        crate_ref: CrateRef,
    ) -> anyhow::Result<Option<ItemLookupIndex>> {
        if self
            .body_ir
            .item_lookup_index(crate_ref)
            .context("read the saved crate item lookup index")?
            .is_some()
        {
            return Ok(None);
        }

        ItemLookupIndex::build_from(&CrateItemQuery::new(
            &self.def_map,
            &self.semantic_ir,
            crate_ref,
        ))
        .map(Some)
        .context("build the request-local crate item lookup index")
    }

    /// Allocate the first request-only body id after this crate's saved body ids.
    pub fn first_synthetic_body_ref(
        &self,
        crate_ref: CrateRef,
    ) -> Result<rg_ir_model::BodyRef, PackageStoreError> {
        self.body_ir.first_synthetic_body_ref(crate_ref)
    }

    /// Add request-local body replacements without changing the saved transactions.
    pub fn with_current_body_set(mut self, current: CurrentBodySet) -> Self {
        self.body_ir = self.body_ir.with_current_body_set(current);
        self
    }

    /// Return whether a body identity belongs to request-local current Body IR.
    pub fn is_current_body(&self, body_ref: BodyRef) -> bool {
        self.body_ir.is_current_body(body_ref)
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

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.def_map.root_module(crate_ref)
    }
}
