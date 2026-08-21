//! Body IR payloads used by resident queries and exact-target cache rewrites.

use rg_arena::Arena;
use rg_def_map::DefMap;
use rg_ir_model::{BodyId, CrateId};
use rg_semantic_ir::ItemStore;
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use super::{
    BodyLocalItems, CrateBodiesCoverage, CrateBodiesManifest, CrateBodiesStatus,
    PackageBodiesCoverage, PackageBodiesManifest,
};
use crate::{BodyData, BodyFacts, BodyView};

/// Body IR payload slots for all semantic crates inside one package.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct PackageBodies {
    pub(crate) crates: Arena<CrateId, CrateBodies>,
}

impl PackageBodies {
    pub fn new(crates: Vec<CrateBodies>) -> Self {
        Self {
            crates: Arena::from_vec(crates),
        }
    }

    pub fn crates(&self) -> &[CrateBodies] {
        self.crates.as_slice()
    }

    pub fn crate_bodies(&self, crate_id: CrateId) -> Option<&CrateBodies> {
        self.crates.get(crate_id)
    }

    /// Build the compact coverage directory that remains resident after this payload is offloaded.
    pub(crate) fn coverage(&self) -> PackageBodiesCoverage {
        PackageBodiesCoverage::from_crates(
            self.crates().iter().map(CrateBodies::coverage).collect(),
        )
    }

    /// Build the temporary package overlay used to improve one target in a cached package.
    ///
    /// The manifests preserve sibling body routing without decoding their file shards. The project
    /// layer must either replace each placeholder or copy its encoded shards into a new artifact
    /// before exposing the package to ordinary Body IR queries.
    pub fn from_cached_manifest(manifest: &PackageBodiesManifest) -> Self {
        Self::new(
            manifest
                .crates()
                .iter()
                .cloned()
                .map(CrateBodies::from_cached_manifest)
                .collect(),
        )
    }

    /// Return whether an artifact rewrite must supply encoded payloads for cached siblings.
    pub fn has_cached_payloads(&self) -> bool {
        self.crates().iter().any(CrateBodies::has_cached_payload)
    }
}

/// Body payload and coverage for one semantic crate.
///
/// Ordinary query state contains decoded body arenas and their aligned semantic sidecars. While an
/// exact target is rebuilt from an offloaded package, an untouched sibling can instead retain only
/// its cache manifest. That cached form is routing data for the artifact rewrite, not an empty body
/// set, so ordinary body access rejects it.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct CrateBodies {
    payload: CrateBodiesPayload,
}

/// The package rewrite overlay must never look like an empty resident crate.
///
/// Keeping coverage inside each variant also prevents a cached manifest from disagreeing with a
/// separately stored coverage field. The cached form exists only while one exact target is rebuilt
/// and its sibling shards remain encoded in the old package artifact.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
enum CrateBodiesPayload {
    Resident {
        coverage: CrateBodiesCoverage,
        bodies: Arena<BodyId, BodyData>,
        facts: Arena<BodyId, BodyFacts>,
        body_local_items: Arena<BodyId, BodyLocalItems>,
    },
    Cached(CrateBodiesManifest),
}

type ResidentCrateBodyArenas<'a> = (
    &'a Arena<BodyId, BodyData>,
    &'a Arena<BodyId, BodyFacts>,
    &'a Arena<BodyId, BodyLocalItems>,
);

impl CrateBodies {
    pub(crate) fn empty(coverage: CrateBodiesCoverage) -> Self {
        debug_assert!(
            !coverage.is_materialized(),
            "materialized crate bodies should be constructed from resolved build output",
        );
        Self::from_resident_parts(coverage, Arena::new(), Arena::new(), Arena::new())
    }

    fn from_cached_manifest(manifest: CrateBodiesManifest) -> Self {
        Self {
            payload: CrateBodiesPayload::Cached(manifest),
        }
    }

    pub(crate) fn from_build(
        coverage: CrateBodiesCoverage,
        bodies: Arena<BodyId, BodyData>,
        facts: Arena<BodyId, BodyFacts>,
        body_local_items: Arena<BodyId, BodyLocalItems>,
    ) -> Self {
        debug_assert!(coverage.is_materialized());
        Self::from_resident_parts(coverage, bodies, facts, body_local_items)
    }

    /// Construct a decoded resident payload after build or cache-shard validation.
    ///
    /// Coverage can still be incomplete here because startup validation deliberately decodes an
    /// incomplete artifact before the project layer rejects its durability marker.
    pub(crate) fn from_resident_parts(
        coverage: CrateBodiesCoverage,
        bodies: Arena<BodyId, BodyData>,
        facts: Arena<BodyId, BodyFacts>,
        body_local_items: Arena<BodyId, BodyLocalItems>,
    ) -> Self {
        debug_assert_eq!(bodies.len(), facts.len());
        debug_assert_eq!(bodies.len(), body_local_items.len());
        debug_assert!(
            bodies
                .iter()
                .zip(&facts)
                .all(|(body, facts)| facts.is_aligned_with(body)),
        );
        Self {
            payload: CrateBodiesPayload::Resident {
                coverage,
                bodies,
                facts,
                body_local_items,
            },
        }
    }

    /// Access the aligned decoded arenas used by ordinary Body IR queries.
    ///
    /// A cached payload is a short-lived artifact-rewrite overlay, not an empty crate. Panicking
    /// here catches accidental publication of that overlay instead of returning false negatives.
    fn resident_arenas(&self) -> ResidentCrateBodyArenas<'_> {
        let CrateBodiesPayload::Resident {
            bodies,
            facts,
            body_local_items,
            ..
        } = &self.payload
        else {
            panic!("cached Body IR payload must be rewritten before ordinary body queries");
        };
        (bodies, facts, body_local_items)
    }

    pub(crate) fn cached_manifest(&self) -> Option<&CrateBodiesManifest> {
        match &self.payload {
            CrateBodiesPayload::Resident { .. } => None,
            CrateBodiesPayload::Cached(manifest) => Some(manifest),
        }
    }

    /// Return whether this slot's body arenas remain encoded in a previous cache artifact.
    pub fn has_cached_payload(&self) -> bool {
        matches!(&self.payload, CrateBodiesPayload::Cached(_))
    }

    pub fn coverage(&self) -> CrateBodiesCoverage {
        match &self.payload {
            CrateBodiesPayload::Resident { coverage, .. } => *coverage,
            CrateBodiesPayload::Cached(manifest) => manifest.coverage(),
        }
    }

    pub fn status(&self) -> CrateBodiesStatus {
        self.coverage().status()
    }

    pub fn body(&self, body: BodyId) -> Option<BodyView<'_>> {
        let (bodies, facts, _) = self.resident_arenas();
        Some(BodyView::new(bodies.get(body)?, facts.get(body)?))
    }

    pub(crate) fn body_facts(&self, body: BodyId) -> Option<&BodyFacts> {
        let (_, facts, _) = self.resident_arenas();
        facts.get(body)
    }

    pub fn body_local_items(&self, body: BodyId) -> Option<&BodyLocalItems> {
        let (_, _, body_local_items) = self.resident_arenas();
        body_local_items.get(body)
    }

    pub fn body_def_map(&self, body: BodyId) -> Option<&DefMap> {
        self.body_local_items(body).map(BodyLocalItems::def_map)
    }

    pub fn body_item_store(&self, body: BodyId) -> Option<&ItemStore> {
        self.body_local_items(body).map(BodyLocalItems::item_store)
    }

    pub fn bodies(&self) -> &[BodyData] {
        let (bodies, _, _) = self.resident_arenas();
        bodies.as_slice()
    }

    /// Iterate bodies in stable `BodyId` order with their aligned semantic facts.
    pub fn body_views(&self) -> impl Iterator<Item = (BodyId, BodyView<'_>)> {
        let (bodies, facts, _) = self.resident_arenas();
        bodies
            .iter_with_ids()
            .map(move |(body, data)| (body, BodyView::new(data, &facts[body])))
    }
}
