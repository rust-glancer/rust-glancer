use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::declare_id;

/// Stable slot of one package inside a normalized workspace metadata snapshot.
///
/// Slots are dense and snapshot-local. Rebuild code must rebuild the whole project when Cargo
/// metadata changes package ordering or membership, so analysis IDs never cross metadata graphs.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    SchemaRead,
    SchemaWrite,
    MemorySize,
    Shrink,
)]
#[memsize(leaf)]
#[shrink(leaf)]
pub struct PackageSlot(pub usize);

declare_id! {
    /// Index of a source file in one package's file table.
    ///
    /// The same physical file can have different IDs in different packages. The owning package
    /// must be known before this index can be used to look up a file.
    pub struct FileId;
}
