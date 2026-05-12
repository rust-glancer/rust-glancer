//! Cache-schema workspace/package metadata.
//!
//! These types are the serializable schema for the workspace cache plan. They intentionally copy
//! the subset of workspace metadata that affects artifact selection instead of retaining
//! Cargo/workspace transport types in the cache format.

use std::path::Path;

use rg_workspace::{PackageId, PackageSlot, PackageSource, RustEdition, TargetKind};
use wincode::{SchemaRead, SchemaWrite};

use super::{Fingerprint, fingerprint};

/// Snapshot-local package slot stored in cache metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, SchemaRead, SchemaWrite)]
pub struct CachedPackageSlot(pub u64);

impl CachedPackageSlot {
    pub(super) fn from_workspace(slot: PackageSlot) -> Self {
        Self(u64::try_from(slot.0).expect("package slot should fit into serialized u64"))
    }
}

/// Stable Cargo package id text stored in cache metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash, derive_more::Display, SchemaRead, SchemaWrite)]
#[display("{_0}")]
pub struct CachedPackageId(pub(crate) String);

impl CachedPackageId {
    pub(super) fn from_workspace(id: &PackageId) -> Self {
        Self(id.to_string())
    }
}

/// UTF-8 path text stored in cache metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash, derive_more::Display, SchemaRead, SchemaWrite)]
#[display("{_0}")]
pub struct CachedPath(pub(crate) String);

impl CachedPath {
    pub(super) fn from_workspace_path(path: &Path) -> Self {
        Self(path.display().to_string())
    }

    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }
}

/// Cargo source kind stored in cache metadata.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, derive_more::Display, SchemaRead, SchemaWrite,
)]
pub enum CachedPackageSource {
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

impl From<PackageSource> for CachedPackageSource {
    fn from(source: PackageSource) -> Self {
        match source {
            PackageSource::Workspace => Self::Workspace,
            PackageSource::Path => Self::Path,
            PackageSource::Registry => Self::Registry,
            PackageSource::SparseRegistry => Self::SparseRegistry,
            PackageSource::Git => Self::Git,
            PackageSource::LocalRegistry => Self::LocalRegistry,
            PackageSource::Directory => Self::Directory,
            PackageSource::Sysroot => Self::Sysroot,
        }
    }
}

/// Rust edition stored in cache metadata.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, derive_more::Display, SchemaRead, SchemaWrite,
)]
pub enum CachedRustEdition {
    #[display("2015")]
    Edition2015,
    #[display("2018")]
    Edition2018,
    #[display("2021")]
    Edition2021,
    #[display("2024")]
    Edition2024,
}

impl From<RustEdition> for CachedRustEdition {
    fn from(edition: RustEdition) -> Self {
        match edition {
            RustEdition::Edition2015 => Self::Edition2015,
            RustEdition::Edition2018 => Self::Edition2018,
            RustEdition::Edition2021 => Self::Edition2021,
            RustEdition::Edition2024 => Self::Edition2024,
        }
    }
}

/// Target kind stored in cache metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash, derive_more::Display, SchemaRead, SchemaWrite)]
pub enum CachedTargetKind {
    #[display("lib")]
    Lib,
    #[display("bin")]
    Bin,
    #[display("example")]
    Example,
    #[display("test")]
    Test,
    #[display("bench")]
    Bench,
    #[display("custom-build")]
    CustomBuild,
    #[display("{_0}")]
    Other(String),
}

impl CachedTargetKind {
    pub(super) fn from_workspace(kind: &TargetKind) -> Self {
        match kind {
            TargetKind::Lib => Self::Lib,
            TargetKind::Bin => Self::Bin,
            TargetKind::Example => Self::Example,
            TargetKind::Test => Self::Test,
            TargetKind::Bench => Self::Bench,
            TargetKind::CustomBuild => Self::CustomBuild,
            TargetKind::Other(kind) => Self::Other(kind.clone()),
        }
    }

    fn sort_order(&self) -> u8 {
        match self {
            Self::Lib => 0,
            Self::Bin => 1,
            Self::Example => 2,
            Self::Test => 3,
            Self::Bench => 4,
            Self::CustomBuild => 5,
            Self::Other(_) => 6,
        }
    }
}

/// Cached view of one package's artifact-selecting metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite)]
pub struct CachedPackage {
    pub package: CachedPackageSlot,
    pub package_id: CachedPackageId,
    pub name: String,
    pub source: CachedPackageSource,
    pub edition: CachedRustEdition,
    pub manifest_path: CachedPath,
    pub targets: Vec<CachedTarget>,
    pub dependencies: Vec<CachedDependency>,
}

impl CachedPackage {
    /// Returns the canonical package fingerprint for one workspace root.
    ///
    /// The workspace root is explicit because Cargo package IDs and source paths can contain
    /// absolute workspace paths that should not become part of the stable cache key.
    pub fn fingerprint(&self, workspace_root: &Path) -> Fingerprint {
        fingerprint::FingerprintBuilder::package_identity(workspace_root, self)
    }
}

/// Target metadata that can affect package-local analysis artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite)]
pub struct CachedTarget {
    pub name: String,
    pub kind: CachedTargetKind,
    pub src_path: CachedPath,
}

impl CachedTarget {
    /// Returns targets in the deterministic order used by cache fingerprints and snapshots.
    pub fn sorted(targets: &[Self]) -> Vec<&Self> {
        let mut targets = targets.iter().collect::<Vec<_>>();
        targets.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
        targets
    }

    fn sort_key(&self) -> (u8, &str, &Path) {
        (
            self.kind.sort_order(),
            self.name.as_str(),
            self.src_path.as_path(),
        )
    }
}

/// Dependency edge metadata that can affect package-local path resolution.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite)]
pub struct CachedDependency {
    pub package_id: CachedPackageId,
    pub name: String,
    pub is_normal: bool,
    pub is_build: bool,
    pub is_dev: bool,
}

impl CachedDependency {
    /// Returns dependencies in the deterministic order used by cache fingerprints and snapshots.
    pub fn sorted(dependencies: &[Self]) -> Vec<&Self> {
        let mut dependencies = dependencies.iter().collect::<Vec<_>>();
        dependencies.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
        dependencies
    }

    fn sort_key(&self) -> (&str, String, bool, bool, bool) {
        (
            self.name.as_str(),
            self.package_id.to_string(),
            self.is_normal,
            self.is_build,
            self.is_dev,
        )
    }
}
