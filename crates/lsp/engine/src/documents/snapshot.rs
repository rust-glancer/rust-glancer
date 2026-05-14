use std::{
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirtyDocumentSnapshotState {
    Clean,
    Dirty(DirtyDocumentSnapshot),
    DirtyWithoutText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirtyDocumentSnapshot {
    path: PathBuf,
    version: Option<i32>,
    fingerprint: TextFingerprint,
    text: Arc<str>,
}

impl DirtyDocumentSnapshot {
    pub(super) fn new(
        path: PathBuf,
        version: Option<i32>,
        fingerprint: TextFingerprint,
        text: Arc<str>,
    ) -> Self {
        Self {
            path,
            version,
            fingerprint,
            text,
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn version(&self) -> Option<i32> {
        self.version
    }

    pub(crate) fn fingerprint(&self) -> TextFingerprint {
        self.fingerprint
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextFingerprint {
    len: usize,
    hash: u64,
}

impl TextFingerprint {
    pub(super) fn new(text: &str) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        Self {
            len: text.len(),
            hash: hasher.finish(),
        }
    }

    pub(crate) fn len(self) -> usize {
        self.len
    }

    pub(super) fn hash(self) -> u64 {
        self.hash
    }
}
