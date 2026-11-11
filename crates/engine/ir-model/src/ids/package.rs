use std::path::{Path, PathBuf};

use rg_std::{MemorySize, NativeOsString, Shrink};
use wincode::{SchemaRead, SchemaWrite};

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

/// A canonical source path within the owning Ajika database.
#[ajika::interned(name = "source-files")]
#[derive(MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub struct FileId {
    native_path: NativeOsString,
}

impl FileId {
    pub fn for_path(interner: &ajika::Interner, canonical_path: &Path) -> ajika::Result<Self> {
        Self::new(
            interner,
            NativeOsString::from_os_str(canonical_path.as_os_str()),
        )
    }

    pub fn path(self, interner: &ajika::Interner) -> ajika::Result<PathBuf> {
        self.native_path(interner)?
            .into_os_string()
            .map(PathBuf::from)
            .ok_or_else(|| ajika::Error::contract("invalid native source path"))
    }
}
