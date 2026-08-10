//! Shared read handle for indexed-data views.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use rg_body_ir::BodyIrReadTxn;
use rg_def_map::DefMapReadTxn;
use rg_def_map::{DefMap, DefMapSource};
use rg_ir_model::{CrateRef, DefMapRef, ModuleRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::SemanticIrReadTxn;
use rg_semantic_ir::{ItemStore, ItemStoreSource};
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
        }
    }

    /// Seed crate-semantic solver state produced while building this exact indexed snapshot.
    ///
    /// The caller owns snapshot coherence and the lifetime policy. The view keeps the same
    /// request-local map used by lazily created sessions, so seeded and newly encountered crates
    /// follow one lookup path.
    pub fn with_trait_selection_sessions(
        self,
        sessions: impl IntoIterator<Item = TraitSelectionSession>,
    ) -> Self {
        {
            let mut by_crate = self
                .trait_selection
                .lock()
                .expect("trait-selection session map lock should not be poisoned");
            for session in sessions {
                by_crate.insert(session.use_site(), session);
            }
        }
        self
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
