use std::path::PathBuf;

use crate::file::FileId;
use rg_arena::ArenaId;
use rg_workspace::TargetKind;

/// Stable identifier of a target within one parsed package.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct TargetId(pub usize);

impl ArenaId for TargetId {
    fn from_index(index: usize) -> Self {
        Self(index)
    }

    fn index(self) -> usize {
        self.0
    }
}

/// Parsed target metadata.
///
/// A single package may define multiple targets, such as `lib.rs`, `main.rs`, examples, or tests.
/// This phase keeps only the normalized target identity and its parsed root source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Stable target id assigned during package parsing.
    pub id: TargetId,
    /// Normalized target name.
    pub name: String,
    /// Normalized target kind.
    pub kind: TargetKind,
    /// Target entrypoint path from workspace metadata.
    pub src_path: PathBuf,
    /// Entrypoint file id for this target.
    pub root_file: FileId,
}

impl Target {
    pub(crate) fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
    }
}
