//! Stable identity for the exact bytes behind one source file.
//!
//! `SourceRevision` is the content identity used while a project generation is alive.
//! `SourceDescriptor` adds the path and byte length needed to carry that identity through a
//! package-cache snapshot. Neither type owns source text.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Strong identity of the exact UTF-8 bytes captured for one source file.
///
/// A revision does not mean "the latest contents at this path". It names the bytes that one
/// project generation used, even if the path changes on disk after the generation is published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub struct SourceRevision([u8; 32]);

impl SourceRevision {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for SourceRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Cache-facing proof that a file belonged to one exact source generation.
///
/// The path identifies which source to capture during cache restoration. The revision and byte
/// length then prove that the captured file is the same input that produced the cached analysis.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct SourceDescriptor {
    path: String,
    revision: SourceRevision,
    byte_len: u64,
}

impl SourceDescriptor {
    pub(crate) fn new(path: PathBuf, bytes: &[u8]) -> Self {
        Self {
            path: path.to_string_lossy().into_owned(),
            revision: SourceRevision::from_bytes(bytes),
            byte_len: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        }
    }

    pub fn path(&self) -> &Path {
        Path::new(&self.path)
    }

    pub fn revision(&self) -> SourceRevision {
        self.revision
    }

    pub fn byte_len(&self) -> u64 {
        self.byte_len
    }
}
