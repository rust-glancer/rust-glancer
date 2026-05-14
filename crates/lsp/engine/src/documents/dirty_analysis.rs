use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use super::{DirtyDocumentSnapshot, DirtyDocumentSnapshotState, TextFingerprint};

/// Shared freshness oracle for dirty-buffer analysis requests.
///
/// The worker queue is FIFO, but document changes arrive outside that queue. This state lets the
/// worker cheaply reject a queued request that was captured from an older dirty buffer version.
#[derive(Debug, Clone, Default)]
pub(crate) struct DirtyAnalysisHandle {
    state: Arc<Mutex<DirtyAnalysisState>>,
}

impl DirtyAnalysisHandle {
    pub(crate) fn sync_document(&self, path: &Path, snapshot: &DirtyDocumentSnapshotState) {
        self.state().sync_document(path, snapshot);
    }

    pub(crate) fn is_current_identity(&self, identity: &DirtyDocumentIdentity) -> bool {
        self.state().is_current(identity)
    }

    fn state(&self) -> MutexGuard<'_, DirtyAnalysisState> {
        self.state
            .lock()
            .expect("dirty analysis state mutex should not be poisoned")
    }
}

#[derive(Debug, Default)]
struct DirtyAnalysisState {
    latest_by_path: HashMap<PathBuf, DirtyDocumentIdentity>,
}

impl DirtyAnalysisState {
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
