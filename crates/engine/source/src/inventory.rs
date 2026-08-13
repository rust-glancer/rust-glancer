//! Source capture and validation for one project-generation candidate.
//!
//! An inventory has a small lifecycle that mirrors project construction:
//!
//! 1. While open, parsing and ItemTree may capture files and remember module-existence checks.
//! 2. After file discovery, the inventory is sealed. Known entries remain readable, but a later
//!    phase cannot add a file that earlier phases never saw.
//! 3. Before publication, validation rereads saved files and repeats existence checks. Successful
//!    validation also releases those construction-only existence checks.
//! 4. After publication, saved text may be evicted while entries keep their revision proof.
//!
//! A saved update starts by forking the published inventory. The maps are independent, but
//! unchanged `SourceEntry` values are shared because an entry's descriptor never changes.
//!
//! A disposable source-override candidate uses the same inventory API with a stricter source
//! universe. It may replace already-known entries with captured text and follow module
//! declarations to other known entries, but it cannot discover a new path from disk. That keeps
//! the derived analysis anchored to the saved generation selected by the caller.

use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
    sync::{Arc, RwLock},
};

use rg_std::{MemoryRecorder, MemorySize};

use crate::{
    CapturedSource, SourceDescriptor, SourceEntry, SourceError, SourcePath, read_source_text,
};

/// Path-indexed source set captured for one project generation.
///
/// The inventory is the authority that decides whether a filesystem read belongs to this
/// generation. Analysis phases may ask it for an already-known entry, but only an open candidate
/// may capture a new path or replace an entry.
#[derive(Debug, Default)]
pub struct SourceInventory {
    entries: RwLock<HashMap<SourcePath, Arc<SourceEntry>>>,
    existence: RwLock<HashMap<PathBuf, bool>>,
    state: RwLock<InventoryState>,
}

/// Construction state: whether the inventory is sealed and which sources it may admit.
#[derive(Debug, Clone, Copy, Default)]
struct InventoryState {
    sealed: bool,
    mode: InventoryMode,
}

/// Source-admission policy for the candidate being built.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum InventoryMode {
    #[default]
    SavedCandidate,
    SourceOverrides,
}

impl SourceInventory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a candidate inventory without giving it access to the published maps.
    ///
    /// The new map initially points at the same immutable entries. Replacing `lib.rs` in the
    /// candidate changes only its map entry; the published inventory continues to point at the
    /// previous revision.
    pub fn fork(&self) -> Self {
        let entries = self
            .entries
            .read()
            .expect("source inventory lock should not be poisoned")
            .clone();
        let state = *self
            .state
            .read()
            .expect("source inventory state lock should not be poisoned");
        Self {
            entries: RwLock::new(entries),
            existence: RwLock::new(
                self.existence
                    .read()
                    .expect("source existence lock should not be poisoned")
                    .clone(),
            ),
            state: RwLock::new(state),
        }
    }

    /// Opens a private candidate for source replacement and another discovery pass.
    ///
    /// Existing entries stay in place because unchanged files still belong to the candidate.
    /// Existence probes are cleared because a new module may have appeared since the published
    /// generation decided that `foo.rs` did not exist.
    pub fn begin_capture(&self) {
        *self
            .state
            .write()
            .expect("source inventory state lock should not be poisoned") = InventoryState {
            sealed: false,
            mode: InventoryMode::SavedCandidate,
        };
        self.existence
            .write()
            .expect("source existence lock should not be poisoned")
            .clear();
    }

    /// Opens a disposable source-override candidate over exactly this inventory's source universe.
    ///
    /// Module discovery may follow declarations to another already-known source, but it must not
    /// admit a file merely because that path happens to exist on disk after the saved generation
    /// was published.
    pub fn begin_source_overrides(&self) {
        *self
            .state
            .write()
            .expect("source inventory state lock should not be poisoned") = InventoryState {
            sealed: false,
            mode: InventoryMode::SourceOverrides,
        };
        self.existence
            .write()
            .expect("source existence lock should not be poisoned")
            .clear();
    }

    /// Ends file discovery for this generation.
    ///
    /// Sealing does not make known source unreadable. It only prevents later phases or query code
    /// from silently adding paths that were absent from the generation's discovery pass.
    pub fn seal(&self) {
        self.state
            .write()
            .expect("source inventory state lock should not be poisoned")
            .sealed = true;
    }

    pub fn is_sealed(&self) -> bool {
        self.state
            .read()
            .expect("source inventory state lock should not be poisoned")
            .sealed
    }

    /// Returns the generation's existing entry or captures a saved file for an open candidate.
    pub fn capture_saved(&self, path: &Path) -> Result<Arc<SourceEntry>, SourceError> {
        if let Some(entry) = self.known_entry(path) {
            return Ok(entry);
        }
        if self.mode() == InventoryMode::SourceOverrides {
            return Err(SourceError::Unknown {
                path: path.to_path_buf(),
            });
        }

        let canonical_path = path.canonicalize().map_err(|source| SourceError::Io {
            path: path.to_path_buf(),
            source,
        })?;

        // Known entries are valid in sealed generations. Only the first capture represents file
        // discovery and therefore needs the inventory to be open.
        if let Some(entry) = self.entry(&canonical_path) {
            return Ok(entry);
        }
        self.ensure_open(&canonical_path)?;

        // Parallel package lowering can race to capture a shared source. Both reads are valid; the
        // first entry inserted becomes the one revision used by this generation.
        let text = read_source_text(&canonical_path)?;
        let path = SourcePath::new(canonical_path);
        self.insert_if_absent(path.clone(), SourceEntry::saved(path, text))
    }

    /// Captures caller-provided text only when `path` belongs to this frozen source inventory.
    pub fn capture_known(&self, path: &Path, text: impl Into<Arc<str>>) -> Option<CapturedSource> {
        let entry = self.known_entry(path)?;
        Some(CapturedSource::from_source_path(
            entry.source_path().clone(),
            text,
        ))
    }

    /// Keeps exact matching captured bytes available to an otherwise unchanged saved generation.
    ///
    /// This only fills the evictable text cell of the existing immutable source identity. It does
    /// not create a source override or change any derived analysis state.
    pub fn retain_matching_text(&self, source: &CapturedSource) -> bool {
        self.entry(source.path())
            .is_some_and(|entry| entry.retain_matching_text(source))
    }

    /// Replaces one saved path in an open candidate using caller-captured exact source.
    pub fn replace_saved(&self, source: &CapturedSource) -> Result<Arc<SourceEntry>, SourceError> {
        self.ensure_open(source.path())?;
        let path = source.source_path().clone();
        let entry = Arc::new(SourceEntry::saved_captured(source));
        self.entries
            .write()
            .expect("source inventory lock should not be poisoned")
            .insert(path, Arc::clone(&entry));
        Ok(entry)
    }

    /// Replaces one saved path by capturing its disk bytes at the start of candidate rebuilding.
    ///
    /// An exact watcher replay keeps the existing immutable entry. Package file tables may still
    /// point at that entry, and retaining it avoids splitting one unchanged source identity across
    /// two otherwise equivalent allocations when another file in the same batch does change.
    pub fn replace_saved_from_disk(
        &self,
        canonical_path: &Path,
    ) -> Result<Arc<SourceEntry>, SourceError> {
        self.ensure_open(canonical_path)?;
        let text = read_source_text(canonical_path)?;
        let path = SourcePath::new(canonical_path.to_path_buf());
        let entry = Arc::new(SourceEntry::saved(path.clone(), text));
        let mut entries = self
            .entries
            .write()
            .expect("source inventory lock should not be poisoned");
        if let Some(existing) = entries.get(&path)
            && existing.is_saved()
            && existing.revision() == entry.revision()
            && existing.byte_len() == entry.byte_len()
        {
            return Ok(Arc::clone(existing));
        }
        entries.insert(path, Arc::clone(&entry));
        Ok(entry)
    }

    /// Replaces one known path with captured text in a source-override candidate.
    pub fn replace_with_override(
        &self,
        source: &CapturedSource,
    ) -> Result<Arc<SourceEntry>, SourceError> {
        self.ensure_open(source.path())?;
        if self.mode() != InventoryMode::SourceOverrides || self.entry(source.path()).is_none() {
            return Err(SourceError::Unknown {
                path: source.path().to_path_buf(),
            });
        }
        let path = source.source_path().clone();
        let entry = Arc::new(SourceEntry::in_memory(path.clone(), source.shared_text()));
        self.entries
            .write()
            .expect("source inventory lock should not be poisoned")
            .insert(path, Arc::clone(&entry));
        Ok(entry)
    }

    /// Captures a cache snapshot's source and proves that its descriptor still matches.
    ///
    /// Cache fingerprints summarize a package, but restoration also checks each file descriptor.
    /// This prevents a matching or forged header from reconnecting derived data to different
    /// source bytes.
    pub fn capture_descriptor(
        &self,
        descriptor: &SourceDescriptor,
    ) -> Result<Arc<SourceEntry>, SourceError> {
        let entry = self.capture_saved(descriptor.path())?;
        if entry.revision() != descriptor.revision() || entry.byte_len() != descriptor.byte_len() {
            return Err(SourceError::Stale {
                path: descriptor.path().to_path_buf(),
                expected: descriptor.revision(),
                actual: entry.revision(),
            });
        }
        Ok(entry)
    }

    pub fn entry(&self, path: &Path) -> Option<Arc<SourceEntry>> {
        self.entries
            .read()
            .expect("source inventory lock should not be poisoned")
            .get(path)
            .cloned()
    }

    /// Returns every saved path whose disk bytes advanced past this frozen inventory.
    ///
    /// A failed incremental build ordinarily reports only the first stale source it tried to read.
    /// Hosts use this scan after an edit burst settles so the next candidate can refresh all known
    /// changed files together instead of discovering them through one failed rebuild at a time.
    pub fn stale_saved_paths(&self) -> Result<Vec<PathBuf>, SourceError> {
        let entries = self
            .entries
            .read()
            .expect("source inventory lock should not be poisoned");
        let mut stale_paths = Vec::new();

        for entry in entries.values() {
            match entry.validate_saved() {
                Ok(()) => {}
                // Project update policy handles deletions through the surviving module or Cargo
                // file that stopped naming this path. A disappeared entry should not hide other
                // changed files from burst reconciliation.
                Err(error) if error.io_kind() == Some(std::io::ErrorKind::NotFound) => {}
                Err(error) => {
                    let Some(path) = error.stale_path() else {
                        return Err(error);
                    };
                    stale_paths.push(path.to_path_buf());
                }
            }
        }

        Ok(stale_paths)
    }

    /// Drops entries that are no longer part of any parsed package file table.
    ///
    /// Package rebuilding can replace its module graph, so a source captured by an older
    /// generation is not automatically part of the new one. Retiring it here keeps final
    /// validation and persistent source snapshots tied to the newly discovered file union.
    pub fn retain_paths(&self, paths: impl IntoIterator<Item = PathBuf>) {
        let paths = paths.into_iter().collect::<HashSet<_>>();
        let mut entries = self
            .entries
            .write()
            .expect("source inventory lock should not be poisoned");
        let previous_len = entries.len();
        entries.retain(|path, _| paths.contains(path.as_path()));
        if entries.len() != previous_len {
            entries.shrink_to_fit();
        }
    }

    /// Remembers one module-discovery decision made by an open candidate.
    ///
    /// Repeated checks return the first answer so one generation cannot observe `foo.rs` as both
    /// missing and present. Final validation checks the answer again before publication.
    pub fn probe_exists(&self, path: &Path) -> Result<bool, SourceError> {
        if let Some(exists) = self
            .existence
            .read()
            .expect("source existence lock should not be poisoned")
            .get(path)
            .copied()
        {
            return Ok(exists);
        }
        self.ensure_open(path)?;
        let exists = if self.mode() == InventoryMode::SourceOverrides {
            self.known_entry(path).is_some()
        } else {
            path.is_file()
        };
        let mut existence = self
            .existence
            .write()
            .expect("source existence lock should not be poisoned");
        Ok(*existence.entry(path.to_path_buf()).or_insert(exists))
    }

    /// Proves that all filesystem observations still match the candidate being published.
    ///
    /// Existence probes are needed only while constructing and validating a generation. Once the
    /// proof succeeds, their paths and map storage are released instead of becoming published
    /// project state.
    pub fn validate_saved(&self) -> Result<(), SourceError> {
        // First prove that every saved file still contains the bytes used by parsing and lowering.
        let entries = self
            .entries
            .read()
            .expect("source inventory lock should not be poisoned");
        for entry in entries.values() {
            entry.validate_saved()?;
        }
        drop(entries);

        self.validate_and_release_existence()
    }

    /// Proves that selected saved files and every module-discovery decision remain valid.
    ///
    /// A source-override project derives new analysis only for its rebuilt packages. Unchanged
    /// package payloads still belong to the already-validated saved generation, so rereading every
    /// source would add filesystem work without strengthening the derived project's consistency.
    pub fn validate_saved_paths<'a>(
        &self,
        paths: impl IntoIterator<Item = &'a Path>,
    ) -> Result<(), SourceError> {
        if self.mode() == InventoryMode::SourceOverrides {
            return self.validate_and_release_existence();
        }
        let entries = self
            .entries
            .read()
            .expect("source inventory lock should not be poisoned");
        let mut validated = HashSet::new();
        for path in paths {
            if validated.insert(path)
                && let Some(entry) = entries.get(path)
            {
                entry.validate_saved()?;
            }
        }
        drop(entries);

        self.validate_and_release_existence()
    }

    fn validate_and_release_existence(&self) -> Result<(), SourceError> {
        // Then repeat module-discovery decisions. A newly created or removed candidate module can
        // change the reachable file graph even when all already-captured files are unchanged. Keep
        // the probes intact on failure so the error still describes the candidate that was checked.
        let mut existence = self
            .existence
            .write()
            .expect("source existence lock should not be poisoned");
        if self.mode() == InventoryMode::SourceOverrides {
            existence.clear();
            existence.shrink_to_fit();
            return Ok(());
        }
        for (path, expected) in existence.iter() {
            let actual = path.is_file();
            if actual != *expected {
                return Err(SourceError::ExistenceChanged {
                    path: path.clone(),
                    expected: *expected,
                    actual,
                });
            }
        }

        // A successfully validated candidate no longer needs its discovery proof. Release both
        // the path values and the hash table allocation before the candidate is published.
        existence.clear();
        existence.shrink_to_fit();
        Ok(())
    }

    /// Releases all saved text while preserving the identity needed for verified reloads.
    pub fn evict_saved_text(&self) {
        let entries = self
            .entries
            .read()
            .expect("source inventory lock should not be poisoned");
        for entry in entries.values() {
            entry.evict_saved_text();
        }
    }

    pub fn shrink_to_fit(&self) {
        self.entries
            .write()
            .expect("source inventory lock should not be poisoned")
            .shrink_to_fit();
        self.existence
            .write()
            .expect("source existence lock should not be poisoned")
            .shrink_to_fit();
    }

    fn ensure_open(&self, path: &Path) -> Result<(), SourceError> {
        if self.is_sealed() {
            return Err(SourceError::Sealed {
                path: path.to_path_buf(),
            });
        }
        Ok(())
    }

    fn mode(&self) -> InventoryMode {
        self.state
            .read()
            .expect("source inventory state lock should not be poisoned")
            .mode
    }

    /// Resolve only spelling differences that do not require filesystem observations.
    fn known_entry(&self, path: &Path) -> Option<Arc<SourceEntry>> {
        if let Some(entry) = self.entry(path) {
            return Some(entry);
        }

        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    if !normalized.pop() {
                        normalized.push(component.as_os_str());
                    }
                }
                Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                    normalized.push(component.as_os_str());
                }
            }
        }
        self.entry(&normalized)
    }

    fn insert_if_absent(
        &self,
        path: SourcePath,
        entry: SourceEntry,
    ) -> Result<Arc<SourceEntry>, SourceError> {
        let mut entries = self
            .entries
            .write()
            .expect("source inventory lock should not be poisoned");
        if let Some(existing) = entries.get(&path) {
            return Ok(Arc::clone(existing));
        }
        let entry = Arc::new(entry);
        entries.insert(path, Arc::clone(&entry));
        Ok(entry)
    }
}

impl Clone for SourceInventory {
    fn clone(&self) -> Self {
        self.fork()
    }
}

impl MemorySize for SourceInventory {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("entries", |recorder| {
            let entries = self
                .entries
                .read()
                .expect("source inventory lock should not be poisoned");
            entries.record_memory_children(recorder);
        });
        recorder.scope("existence", |recorder| {
            let existence = self
                .existence
                .read()
                .expect("source existence lock should not be poisoned");
            existence.record_memory_children(recorder);
        });
    }
}
