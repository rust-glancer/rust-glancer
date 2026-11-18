//! Stable identity for the exact bytes behind one source file.
//!
//! `SourceRevision` is the content identity used by one input revision.
//! `SourceDescriptor` adds the path and byte length needed to carry that identity through a
//! source capture. Neither type owns source text.

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use rg_std::MemorySize;
use twox_hash::XxHash3_128;

/// Fingerprint of the UTF-8 bytes captured for one source file.
///
/// A revision does not mean "the latest contents at this path". It names the bytes that one
/// input revision used, even if the path changes on disk after the bytes were captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, MemorySize)]
#[memsize(leaf)]
pub struct SourceRevision(u128);

impl SourceRevision {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(XxHash3_128::oneshot(bytes))
    }

    pub(crate) fn stamp(self) -> ajika::Stamp {
        // Ajika stores 32-byte stamps. Only the first 16 bytes carry the fingerprint.
        let mut bytes = [0; 32];
        bytes[..16].copy_from_slice(&self.0.to_be_bytes());
        ajika::Stamp::from_bytes(bytes)
    }
}

impl fmt::Display for SourceRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

/// Metadata shared by a source capture and the corresponding source input.
#[derive(Debug, Clone, PartialEq, Eq, MemorySize)]
pub(crate) struct SourceDescriptor {
    path: Arc<Path>,
    revision: SourceRevision,
    byte_len: u64,
}

impl SourceDescriptor {
    pub(crate) fn relocated(&self) -> Self {
        Self {
            path: Arc::from(self.path()),
            revision: self.revision,
            byte_len: self.byte_len,
        }
    }

    pub(crate) fn new(path: PathBuf, bytes: &[u8]) -> Self {
        Self {
            path: path.into(),
            revision: SourceRevision::from_bytes(bytes),
            byte_len: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub fn revision(&self) -> SourceRevision {
        self.revision
    }

    pub fn byte_len(&self) -> u64 {
        self.byte_len
    }
}
