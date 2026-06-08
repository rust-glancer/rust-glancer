use std::path::{Path, PathBuf};

use rg_cfg_eval::CfgOptions;
use rg_memsize::MemorySize;

use crate::SysrootCrate;

use super::{dependency::PackageDependency, edition::RustEdition, target::Target};

/// Stable package identifier inside a normalized workspace metadata snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Hash, derive_more::Display, MemorySize)]
#[display("{_0}")]
pub struct PackageId(#[memsize(inline)] pub(crate) String);

impl PackageId {
    pub(crate) fn sysroot(krate: SysrootCrate) -> Self {
        Self(format!("sysroot:{}", krate.name()))
    }
}

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
    wincode::SchemaRead,
    wincode::SchemaWrite,
    MemorySize,
)]
#[memsize(leaf)]
pub struct PackageSlot(pub usize);

/// Where one normalized package came from.
#[derive(Debug, Clone, PartialEq, Eq, MemorySize)]
pub enum PackageOrigin {
    Workspace,
    Dependency,
    Sysroot(SysrootCrate),
}

impl PackageOrigin {
    pub fn is_sysroot(&self) -> bool {
        matches!(self, Self::Sysroot(_))
    }
}

/// Package source kind used for future residency/cache policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, derive_more::Display, MemorySize)]
#[memsize(leaf)]
pub enum PackageSource {
    #[display("workspace")]
    Workspace,
    #[display("path")]
    Path,
    #[display("registry")]
    Registry,
    #[display("sparse-registry")]
    SparseRegistry,
    #[display("git")]
    Git,
    #[display("local-registry")]
    LocalRegistry,
    #[display("directory")]
    Directory,
    #[display("sysroot")]
    Sysroot,
}

/// Normalized package metadata relevant to later analysis phases.
#[derive(Debug, Clone, PartialEq, Eq, MemorySize)]
pub struct Package {
    pub id: PackageId,
    pub name: String,
    pub edition: RustEdition,
    pub origin: PackageOrigin,
    pub source: PackageSource,
    pub is_workspace_member: bool,
    pub manifest_path: PathBuf,
    pub cfg_options: CfgOptions,
    pub targets: Vec<Target>,
    pub dependencies: Vec<PackageDependency>,
}

impl Package {
    /// Returns the package root directory, modeled as the parent of `Cargo.toml`.
    pub fn root_dir(&self) -> &Path {
        self.manifest_path
            .parent()
            .expect("package manifest path should have a parent directory")
    }

    pub(crate) fn contains_path(&self, path: &Path) -> bool {
        path.starts_with(self.root_dir())
    }
}
