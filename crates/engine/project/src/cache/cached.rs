//! Cache-schema workspace/package metadata.
//!
//! These types are the serializable schema for the workspace cache plan. They intentionally copy
//! the subset of workspace metadata that affects artifact selection instead of retaining
//! Cargo/workspace transport types in the cache format.

use std::path::{Component, Path};

use rg_cfg_eval::CfgOptions;
use rg_std::{MemorySize, NativeOsString};
use rg_text::RustEdition;
use rg_workspace::{PackageSlot, PackageSource, TargetKind};
use wincode::{SchemaRead, SchemaWrite};

use super::{Fingerprint, fingerprint};

/// Snapshot-local package slot stored in cache metadata.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, SchemaRead, SchemaWrite, MemorySize,
)]
#[memsize(leaf)]
pub struct CachedPackageSlot(pub u64);

impl CachedPackageSlot {
    pub(super) fn from_workspace(slot: PackageSlot) -> Self {
        Self(u64::try_from(slot.0).expect("package slot should fit into serialized u64"))
    }
}

/// Structural path stored in cache metadata without display-text or separator conversions.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, SchemaRead, SchemaWrite, MemorySize,
)]
pub enum CachedPath {
    /// Components below the normalized workspace root. Component boundaries make this independent
    /// from the host path separator and from the workspace's absolute checkout location.
    WorkspaceRelative(Vec<NativeOsString>),
    /// A host-local absolute path, used for registry, git, path, and sysroot dependencies.
    NativeAbsolute(NativeOsString),
}

impl CachedPath {
    pub(super) fn from_workspace_path(workspace_root: &Path, path: &Path) -> Self {
        if let Ok(relative) = path.strip_prefix(workspace_root) {
            let mut components = Vec::new();
            let mut is_plain_relative = true;
            for component in relative.components() {
                let Component::Normal(component) = component else {
                    is_plain_relative = false;
                    break;
                };
                components.push(NativeOsString::from_os_str(component));
            }
            if is_plain_relative {
                return Self::WorkspaceRelative(components);
            }
        }

        Self::NativeAbsolute(NativeOsString::from_os_str(path.as_os_str()))
    }

    /// Reconstructs the producing host's path, rejecting incompatible or malformed cache data.
    #[cfg(test)]
    pub(super) fn to_path_buf(&self, workspace_root: &Path) -> Option<std::path::PathBuf> {
        match self {
            Self::WorkspaceRelative(components) => {
                let mut path = workspace_root.to_path_buf();
                for component in components {
                    path.push(component.clone().into_os_string()?);
                }
                Some(path)
            }
            Self::NativeAbsolute(path) => Some(path.clone().into_os_string()?.into()),
        }
    }
}

/// Cargo source kind stored in cache metadata.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    derive_more::Display,
    SchemaRead,
    SchemaWrite,
    MemorySize,
)]
#[memsize(leaf)]
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
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    derive_more::Display,
    SchemaRead,
    SchemaWrite,
    MemorySize,
)]
#[memsize(leaf)]
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
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, derive_more::Display, SchemaRead, SchemaWrite, MemorySize,
)]
pub enum CachedTargetKind {
    #[display("lib")]
    Lib,
    #[display("proc-macro")]
    ProcMacro,
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
            TargetKind::ProcMacro => Self::ProcMacro,
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
            Self::ProcMacro => 1,
            Self::Bin => 2,
            Self::Example => 3,
            Self::Test => 4,
            Self::Bench => 5,
            Self::CustomBuild => 6,
            Self::Other(_) => 7,
        }
    }
}

/// Active cfg facts that influence package-local analysis artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, SchemaRead, SchemaWrite)]
pub struct CachedCfgOptions {
    atoms: Vec<String>,
    key_values: Vec<CachedCfgKeyValue>,
}

impl CachedCfgOptions {
    pub(super) fn from_workspace(options: &CfgOptions) -> Self {
        Self {
            atoms: options.atoms().to_vec(),
            key_values: options
                .key_values()
                .iter()
                .map(|value| CachedCfgKeyValue {
                    key: value.key().to_string(),
                    value: value.value().to_string(),
                })
                .collect(),
        }
    }

    pub(super) fn atoms(&self) -> &[String] {
        &self.atoms
    }

    pub(super) fn key_values(&self) -> &[CachedCfgKeyValue] {
        &self.key_values
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite)]
pub struct CachedCfgKeyValue {
    pub key: String,
    pub value: String,
}

impl CachedCfgKeyValue {
    /// Returns cfg key-values in the deterministic order used by cache fingerprints.
    pub(super) fn sorted(key_values: &[Self]) -> Vec<&Self> {
        let mut key_values = key_values.iter().collect::<Vec<_>>();
        key_values.sort_by(|left, right| {
            left.key
                .cmp(&right.key)
                .then_with(|| left.value.cmp(&right.value))
        });
        key_values
    }
}

/// Cached view of one package's artifact-selecting metadata.
///
/// The passively selected Cargo output directory, generated-file inventory, and compile-time
/// environment are intentionally absent. They help a source build discover one usable snapshot,
/// but they do not make another still-valid historical snapshot semantically unusable. The
/// package artifact's source fingerprint separately validates every file that snapshot parsed.
/// Recovered build-script cfg remains in `cfg_options` because it controls which source is active.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct CachedPackage {
    pub package: CachedPackageSlot,
    pub name: String,
    pub source: CachedPackageSource,
    pub edition: CachedRustEdition,
    pub manifest_path: CachedPath,
    #[memsize(skip)]
    pub cfg_options: CachedCfgOptions,
    pub targets: Vec<CachedTarget>,
    pub dependencies: Vec<CachedDependency>,
}

impl CachedPackage {
    /// Returns the canonical fingerprint for this already-structured package identity.
    pub fn fingerprint(&self) -> Fingerprint {
        fingerprint::FingerprintBuilder::package_identity(self)
    }
}

/// Target metadata that can affect package-local analysis artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
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

    fn sort_key(&self) -> (u8, &str, &CachedPath) {
        (self.kind.sort_order(), self.name.as_str(), &self.src_path)
    }
}

/// Dependency edge metadata that can affect package-local path resolution.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct CachedDependency {
    pub package: CachedPackageSlot,
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

    fn sort_key(&self) -> (&str, CachedPackageSlot, bool, bool, bool) {
        (
            self.name.as_str(),
            self.package,
            self.is_normal,
            self.is_build,
            self.is_dev,
        )
    }
}
