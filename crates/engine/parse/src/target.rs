use std::path::PathBuf;

use crate::file::FileId;
use rg_arena::ArenaId;
use rg_std::{MemorySize, Shrink};
use rg_workspace::TargetKind;
use wincode::{SchemaRead, SchemaWrite};

/// Stable identifier of a Cargo target within one parsed package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub struct CargoTargetId(pub usize);

impl ArenaId for CargoTargetId {
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
#[derive(Debug, Clone, PartialEq, Eq, MemorySize)]
pub struct CargoTarget {
    /// Stable target id assigned during package parsing.
    pub id: CargoTargetId,
    /// Normalized target name.
    pub name: String,
    /// Normalized target kind.
    pub kind: TargetKind,
    /// Target entrypoint path from workspace metadata.
    pub src_path: PathBuf,
    /// Entrypoint file id for this target.
    pub root_file: FileId,
}

impl CargoTarget {
    /// Returns whether this target should enable `cfg(test)` during analysis.
    pub fn enables_test_cfg(&self) -> bool {
        self.kind.enables_test_cfg()
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
    }
}
