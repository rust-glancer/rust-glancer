//! Cache-facing manifest and source-file shards.
//!
//! Stable body ids are dense within a crate, but bodies from the same file are not necessarily
//! adjacent. The manifest keeps the body-to-file routing table, while each shard repeats the body
//! id beside its data. Reconstruction validates both sides before rebuilding the dense arenas, so
//! malformed cache bytes cannot silently attach a body to the wrong id.
//!
//! For example, a crate may have this dense resident order:
//!
//! ```text
//! BodyId(0) -> lib.rs
//! BodyId(1) -> foo.rs
//! BodyId(2) -> lib.rs
//! ```
//!
//! The manifest stores `[lib.rs, foo.rs, lib.rs]`, while the `lib.rs` shard stores entries for body
//! 0 and body 2. Loading that shard is enough to answer queries in `lib.rs`; loading all shards and
//! placing entries back into their recorded body slots recreates the resident arenas.

use anyhow::Context as _;
use rg_arena::{Arena, ArenaId as _};
use rg_ir_model::{BodyData, BodyId, CrateId};
use rg_parse::FileId;
use rg_semantic_ir::ItemLookupIndex;
use rg_std::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

use super::{BodyLocalItems, CrateBodies, CrateBodiesCoverage, PackageBodies};
use crate::{BodyFacts, BodyView};

impl PackageBodies {
    /// Build the small package directory read before any crate payload.
    pub fn manifest(&self) -> PackageBodiesManifest {
        PackageBodiesManifest {
            crates: Arena::from_vec(self.crates().iter().map(CrateBodies::manifest).collect()),
        }
    }
}

/// Package-level Body IR directory used before any crate index or body payload is decoded.
///
/// This deliberately contains only crate manifests. Crate semantic indexes and file shards are
/// separate payloads in the cache container.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct PackageBodiesManifest {
    crates: Arena<CrateId, CrateBodiesManifest>,
}

impl PackageBodiesManifest {
    pub fn crates(&self) -> &[CrateBodiesManifest] {
        self.crates.as_slice()
    }

    pub fn crate_manifest(&self, crate_id: CrateId) -> Option<&CrateBodiesManifest> {
        self.crates.get(crate_id)
    }
}

/// Routing information needed to load one crate in source-file-sized pieces.
///
/// `body_files` supports direct `BodyId -> FileId` lookup. `files` stores the sorted unique file
/// list so a crate-wide query can enumerate every shard without scanning the body mapping first.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct CrateBodiesManifest {
    coverage: CrateBodiesCoverage,
    body_files: Arena<BodyId, FileId>,
    files: Vec<FileId>,
}

impl CrateBodiesManifest {
    pub fn coverage(&self) -> CrateBodiesCoverage {
        self.coverage
    }

    pub fn body_count(&self) -> usize {
        self.body_files.len()
    }

    pub fn body_file(&self, body: BodyId) -> Option<FileId> {
        self.body_files.get(body).copied()
    }

    pub fn files(&self) -> &[FileId] {
        &self.files
    }
}

/// Bodies and body-local item stores originating in one package-local source file.
///
/// A shard is the smallest independently decoded Body IR payload. The entries keep their stable
/// `BodyId`s because their positions inside this vector do not match positions in the resident
/// crate arena.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct BodyFileShard {
    file: FileId,
    entries: Vec<BodyFileEntry>,
}

impl BodyFileShard {
    pub fn file(&self) -> FileId {
        self.file
    }

    pub fn entries(&self) -> &[BodyFileEntry] {
        &self.entries
    }

    pub fn body(&self, body: BodyId) -> Option<BodyView<'_>> {
        self.entries
            .iter()
            .find(|entry| entry.body == body)
            .map(BodyFileEntry::view)
    }

    pub fn body_local_items(&self, body: BodyId) -> Option<&BodyLocalItems> {
        self.entries
            .iter()
            .find(|entry| entry.body == body)
            .map(|entry| &entry.local_items)
    }
}

/// One stable body id and the payloads that must move with it.
///
/// `BodyData`, `BodyFacts`, and `BodyLocalItems` use the same `BodyId` in the resident
/// representation. Keeping them in one shard entry prevents independently decoded arrays from
/// drifting apart.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct BodyFileEntry {
    body: BodyId,
    data: BodyData,
    facts: BodyFacts,
    local_items: BodyLocalItems,
}

impl BodyFileEntry {
    pub fn body(&self) -> BodyId {
        self.body
    }

    pub fn view(&self) -> BodyView<'_> {
        BodyView::new(&self.data, &self.facts)
    }

    pub fn local_items(&self) -> &BodyLocalItems {
        &self.local_items
    }
}

impl CrateBodies {
    /// Build the directory that routes dense body ids to file shards.
    ///
    /// The `body_files` arena preserves one entry per body id. The separate file list is sorted and
    /// deduplicated to make serialized output deterministic and shard iteration straightforward.
    pub fn manifest(&self) -> CrateBodiesManifest {
        debug_assert_eq!(
            self.bodies.len(),
            self.facts.len(),
            "every built body should have paired semantic facts",
        );
        debug_assert_eq!(
            self.bodies.len(),
            self.body_local_items.len(),
            "every built body should have paired body-local items",
        );
        let body_files = self
            .bodies
            .iter()
            .map(|body| body.source().file_id)
            .collect::<Vec<_>>();
        let mut files = body_files.clone();
        files.sort_by_key(|file| file.0);
        files.dedup();

        CrateBodiesManifest {
            coverage: self.coverage,
            body_files: Arena::from_vec(body_files),
            files,
        }
    }

    /// Copy one source file's bodies into an independently serializable shard.
    ///
    /// Continuing the module example, asking for `lib.rs` returns entries for body 0 and body 2,
    /// each paired with its body-local items. The stable ids are not renumbered.
    pub fn file_shard(&self, file: FileId) -> BodyFileShard {
        let entries = self
            .bodies
            .iter_with_ids()
            .filter(|(_, body)| body.source().file_id == file)
            .map(|(body, data)| BodyFileEntry {
                body,
                data: data.clone(),
                facts: self
                    .facts
                    .get(body)
                    .expect("every built body should have paired semantic facts")
                    .clone(),
                local_items: self
                    .body_local_items
                    .get(body)
                    .expect("every built body should have paired body-local items")
                    .clone(),
            })
            .collect();
        BodyFileShard { file, entries }
    }

    /// Reassemble the ordinary dense crate representation after loading all of its shards.
    ///
    /// The cache is disposable, so this function rejects any disagreement instead of trying to
    /// recover partial data. Every shard must name a declared file, every body must be in the file
    /// recorded by the manifest, and every dense body slot must be filled exactly once.
    pub fn from_storage_parts(
        manifest: &CrateBodiesManifest,
        semantic_index: ItemLookupIndex,
        shards: Vec<BodyFileShard>,
    ) -> anyhow::Result<Self> {
        // Start with the final dense shape, but leave every slot empty. Shard entries carry stable
        // body ids, so they can be placed directly rather than appended in file order.
        let mut bodies = Vec::with_capacity(manifest.body_count());
        let mut facts = Vec::with_capacity(manifest.body_count());
        let mut body_local_items = Vec::with_capacity(manifest.body_count());
        bodies.resize_with(manifest.body_count(), || None);
        facts.resize_with(manifest.body_count(), || None);
        body_local_items.resize_with(manifest.body_count(), || None);

        // Validate both sides of the routing relationship while filling the dense slots. Checking
        // duplicates here also prevents one later shard from silently overwriting an earlier one.
        for shard in shards {
            anyhow::ensure!(
                manifest.files.contains(&shard.file),
                "Body IR shard belongs to unknown file {:?}",
                shard.file,
            );
            for entry in shard.entries {
                let body_idx = entry.body.index();
                anyhow::ensure!(
                    manifest.body_file(entry.body) == Some(shard.file),
                    "Body IR body {:?} is stored in the wrong file shard {:?}",
                    entry.body,
                    shard.file,
                );
                let body_slot = bodies
                    .get_mut(body_idx)
                    .with_context(|| format!("Body IR shard has unknown body {:?}", entry.body))?;
                anyhow::ensure!(
                    body_slot.is_none(),
                    "Body IR body {:?} is duplicated",
                    entry.body
                );
                anyhow::ensure!(
                    entry.facts.is_aligned_with(&entry.data),
                    "Body IR facts are not aligned with body {:?}",
                    entry.body,
                );
                *body_slot = Some(entry.data);
                facts[body_idx] = Some(entry.facts);
                body_local_items[body_idx] = Some(entry.local_items);
            }
        }

        // A complete shard set must leave no holes. Only after that check do we rebuild the arenas
        // expected by ordinary resident Body IR code.
        let bodies = bodies
            .into_iter()
            .enumerate()
            .map(|(body, data)| {
                data.with_context(|| format!("Body IR shard set is missing body {body}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let body_local_items = body_local_items
            .into_iter()
            .enumerate()
            .map(|(body, data)| {
                data.with_context(|| {
                    format!("Body IR shard set is missing local items for body {body}")
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let facts = facts
            .into_iter()
            .enumerate()
            .map(|(body, data)| {
                data.with_context(|| format!("Body IR shard set is missing facts for body {body}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(Self {
            coverage: manifest.coverage,
            semantic_index,
            bodies: Arena::from_vec(bodies),
            facts: Arena::from_vec(facts),
            body_local_items: Arena::from_vec(body_local_items),
        })
    }
}
