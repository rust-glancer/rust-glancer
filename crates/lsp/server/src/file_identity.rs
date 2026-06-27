//! Lightweight disk identity for saved-file watcher filtering.
//!
//! This is deliberately metadata-based. It may occasionally report a false positive after a
//! metadata-only touch, but it keeps watcher filtering cheap and leaves exact source reads to the
//! engine when a path is actually forwarded.

use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// File metadata precise enough to suppress repeated saved-file notifications.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    len: u64,
    modified: SystemTime,
}

impl FileIdentity {
    pub(crate) fn read(path: &Path) -> Option<(PathBuf, Self)> {
        let metadata = fs::metadata(path).ok()?;
        if !metadata.is_file() {
            return None;
        }

        Some((
            path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
            Self {
                len: metadata.len(),
                modified: metadata.modified().ok()?,
            },
        ))
    }
}
