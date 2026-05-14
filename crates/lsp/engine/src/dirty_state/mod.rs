//! Dirty buffers are modeled as a narrow layer over the saved project state.
//!
//! `DirtyState` is shared with the worker so queued requests can notice when a newer dirty
//! document identity exists and skip obsolete work. Current dirty requests use `DirtyOverlayCache`
//! to build a temporary project overlay with the changed file partially reindexed, keeping the
//! saved project frozen while hover, inlay hints, and similar features read from the buffer text.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

pub(crate) use self::overlay::DirtyOverlayCache;
use crate::documents::{DirtyDocumentSnapshot, DirtyDocumentSnapshotState, TextFingerprint};

mod overlay;

#[cfg(test)]
mod tests;

/// Worker-visible dirty document state for skipping obsolete queued requests.
///
/// `DocumentStore` owns the live text. This is the small synchronous read model the worker uses
/// without waiting behind the analysis command queue.
#[derive(Debug, Clone, Default)]
pub(crate) struct DirtyState {
    state: Arc<Mutex<DirtyStateInner>>,
}

impl DirtyState {
    pub(crate) fn sync_document(&self, path: &Path, snapshot: &DirtyDocumentSnapshotState) {
        self.state().sync_document(path, snapshot);
    }

    pub(crate) fn is_current_identity(&self, identity: &DirtyDocumentIdentity) -> bool {
        self.state().is_current(identity)
    }

    fn state(&self) -> MutexGuard<'_, DirtyStateInner> {
        self.state
            .lock()
            .expect("dirty state mutex should not be poisoned")
    }
}

#[derive(Debug, Default)]
struct DirtyStateInner {
    latest_by_path: HashMap<PathBuf, DirtyDocumentIdentity>,
}

impl DirtyStateInner {
    fn sync_document(&mut self, path: &Path, snapshot: &DirtyDocumentSnapshotState) {
        match snapshot {
            DirtyDocumentSnapshotState::Dirty(snapshot) => {
                self.latest_by_path.insert(
                    snapshot.path().to_path_buf(),
                    DirtyDocumentIdentity::from_snapshot(snapshot),
                );
            }
            DirtyDocumentSnapshotState::Clean | DirtyDocumentSnapshotState::DirtyWithoutText => {
                self.latest_by_path.remove(path);
            }
        }
    }

    fn is_current(&self, identity: &DirtyDocumentIdentity) -> bool {
        self.latest_by_path
            .get(identity.path())
            .is_some_and(|current| current == identity)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirtyDocumentIdentity {
    path: PathBuf,
    version: Option<i32>,
    fingerprint: TextFingerprint,
}

impl DirtyDocumentIdentity {
    pub(crate) fn from_snapshot(snapshot: &DirtyDocumentSnapshot) -> Self {
        Self {
            path: snapshot.path().to_path_buf(),
            version: snapshot.version(),
            fingerprint: snapshot.fingerprint(),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn version(&self) -> Option<i32> {
        self.version
    }

    pub(crate) fn text_len(&self) -> usize {
        self.fingerprint.len()
    }
}
