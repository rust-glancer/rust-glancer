//! Lightweight disk identity for saved-file watcher filtering.
//!
//! This is deliberately metadata-based. It may occasionally report a false positive after a
//! metadata-only touch, but it keeps watcher filtering cheap and leaves exact source reads to the
//! engine when a path is actually forwarded.

use std::{fs, path::Path, time::SystemTime};

use rg_std::NormalizedPathBuf;

/// File metadata precise enough to suppress repeated saved-file notifications.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    len: u64,
    modified: SystemTime,
}

impl FileIdentity {
    pub(crate) fn read(path: &Path) -> Option<(NormalizedPathBuf, Self)> {
        let path = NormalizedPathBuf::from_absolute(path).ok()?;
        let identity = Self::read_normalized(&path)?;
        Some((path, identity))
    }

    pub(crate) fn read_normalized(path: &NormalizedPathBuf) -> Option<Self> {
        let metadata = fs::metadata(path.as_path()).ok()?;
        if !metadata.is_file() {
            return None;
        }

        Some(Self {
            len: metadata.len(),
            modified: metadata.modified().ok()?,
        })
    }
}
