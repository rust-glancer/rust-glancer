//! Filesystem storage for package cache artifacts.
//!
//! This module owns paths and atomic file replacement. Project-level code still owns invalidation:
//! the store can load bytes for an already-vetted header, but it does not decide whether a package
//! should be resident, rebuilt, or evicted.

use std::{
    ffi::OsStr,
    fmt, fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use anyhow::Context as _;
use atomic_write_file::AtomicWriteFile;
use rg_package_store::{MalformedCacheError, PackageStoreError};
use rg_workspace::WorkspaceMetadata;

use super::{
    CachedPackage, Fingerprint, PackageCacheArtifact, PackageCacheCodec, PackageCacheHeader,
    WorkspaceCachePlan,
};

const CACHE_DIR_NAME: &str = "rust_glancer";
const CACHE_PACKAGES_DIR_NAME: &str = "packages";
const CACHE_GENERATION_DIR_PREFIX: &str = "graph-";
const PACKAGE_ARTIFACT_EXTENSION: &str = "rgpkg";

/// Root and naming policy for package cache artifacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageCacheStore {
    workspace_root: PathBuf,
    root: PathBuf,
    generation: Fingerprint,
}

/// Typed failure from reading a package artifact file.
#[derive(Debug)]
pub(crate) enum PackageCacheReadError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Malformed {
        source: MalformedCacheError,
    },
}

impl PackageCacheReadError {
    pub(crate) fn into_package_store_error(
        self,
        slot: rg_workspace::PackageSlot,
    ) -> PackageStoreError {
        match self {
            Self::Io { path, source } => PackageStoreError::io(slot, path, source),
            Self::Malformed { source } => PackageStoreError::malformed_cache(slot, source),
        }
    }
}

impl fmt::Display for PackageCacheReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, .. } => {
                write!(
                    f,
                    "failed to read package cache artifact {}",
                    path.display()
                )
            }
            Self::Malformed { source } => write!(f, "{source}"),
        }
    }
}

impl std::error::Error for PackageCacheReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Malformed { source } => Some(source),
        }
    }
}

impl PackageCacheStore {
    /// Plans cache paths for a workspace using Cargo's target directory convention.
    pub fn for_workspace(workspace: &WorkspaceMetadata, cache_plan: &WorkspaceCachePlan) -> Self {
        let target_dir = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace.workspace_root().join("target"));

        Self::for_workspace_with_target_dir(workspace, cache_plan, target_dir)
    }

    /// Plans cache paths under an explicit Cargo target directory.
    pub(super) fn for_workspace_with_target_dir(
        workspace: &WorkspaceMetadata,
        cache_plan: &WorkspaceCachePlan,
        target_dir: impl Into<PathBuf>,
    ) -> Self {
        let workspace_name = workspace
            .workspace_root()
            .file_name()
            .unwrap_or_else(|| OsStr::new("workspace"));

        Self {
            workspace_root: workspace.workspace_root().to_path_buf(),
            root: target_dir.into().join(CACHE_DIR_NAME).join(workspace_name),
            generation: cache_plan.fingerprint(workspace.workspace_root()),
        }
    }

    #[cfg(test)]
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub fn package_artifact_path(&self, package: &CachedPackage) -> PathBuf {
        let fingerprint = self.package_fingerprint(package);
        let file_name = format!(
            "package-{}-{}-{}.{}",
            package.package.0, package.name, fingerprint, PACKAGE_ARTIFACT_EXTENSION,
        );

        self.generation_dir().join(file_name)
    }

    pub fn package_fingerprint(&self, package: &CachedPackage) -> Fingerprint {
        package.fingerprint(&self.workspace_root)
    }

    /// Removes cache data that cannot be reached through the current workspace graph generation.
    ///
    /// The store deliberately does not track individual artifacts. A source-only save rewrites the
    /// affected package files inside the same generation directory, while Cargo graph changes pick
    /// a new generation and make the older directories disposable.
    pub(crate) fn cleanup_stale_generations(&self) -> anyhow::Result<()> {
        let packages_dir = self.packages_dir();
        let entries = match fs::read_dir(&packages_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "while attempting to read package cache directory {}",
                        packages_dir.display(),
                    )
                });
            }
        };
        let current_generation = self.generation_dir_name();

        for entry in entries {
            let entry = entry.with_context(|| {
                format!(
                    "while attempting to inspect package cache directory {}",
                    packages_dir.display(),
                )
            })?;
            let path = entry.path();
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            let file_type = entry.file_type().with_context(|| {
                format!(
                    "while attempting to inspect package cache entry {}",
                    path.display(),
                )
            })?;

            if file_type.is_dir()
                && file_name.starts_with(CACHE_GENERATION_DIR_PREFIX)
                && file_name != current_generation
            {
                fs::remove_dir_all(&path).with_context(|| {
                    format!(
                        "while attempting to remove stale package cache generation {}",
                        path.display(),
                    )
                })?;
            }
        }

        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn write_artifact(&self, artifact: &PackageCacheArtifact) -> anyhow::Result<()> {
        self.prepare_artifact_writes()?.write_artifact(artifact)
    }

    pub(crate) fn prepare_artifact_writes(&self) -> anyhow::Result<PreparedPackageCacheWriter<'_>> {
        let package_dir = self.generation_dir();
        fs::create_dir_all(&package_dir).with_context(|| {
            format!(
                "while attempting to create package cache directory {}",
                package_dir.display(),
            )
        })?;

        Ok(PreparedPackageCacheWriter { store: self })
    }

    pub fn read_artifact(
        &self,
        header: &PackageCacheHeader,
    ) -> Result<Option<PackageCacheArtifact>, PackageCacheReadError> {
        let artifact = self.read_artifact_for_package(&header.package)?;
        let Some(artifact) = artifact else {
            return Ok(None);
        };

        if artifact.header != *header {
            let path = self.package_artifact_path(&header.package);
            return Err(PackageCacheReadError::Malformed {
                source: MalformedCacheError::HeaderMismatch {
                    path,
                    actual_slot: artifact.header.package.package.0,
                    actual_name: artifact.header.package.name,
                    expected_slot: header.package.package.0,
                    expected_name: header.package.name.clone(),
                },
            });
        }

        Ok(Some(artifact))
    }

    pub(crate) fn read_artifact_for_package(
        &self,
        package: &CachedPackage,
    ) -> Result<Option<PackageCacheArtifact>, PackageCacheReadError> {
        let path = self.package_artifact_path(package);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(PackageCacheReadError::Io { path, source }),
        };

        let artifact = PackageCacheCodec::decode_artifact(&bytes).map_err(|error| {
            PackageCacheReadError::Malformed {
                source: MalformedCacheError::Decode {
                    path: path.clone(),
                    reason: format!("{error:#}"),
                },
            }
        })?;

        if artifact.header.package != *package {
            return Err(PackageCacheReadError::Malformed {
                source: MalformedCacheError::HeaderMismatch {
                    path,
                    actual_slot: artifact.header.package.package.0,
                    actual_name: artifact.header.package.name,
                    expected_slot: package.package.0,
                    expected_name: package.name.clone(),
                },
            });
        }

        Ok(Some(artifact))
    }

    /// Removes this workspace's cache namespace.
    ///
    /// This intentionally never reaches outside `<target>/rust_glancer/<workspace>`; callers can
    /// use it after schema or deserialization failures without touching Cargo's own build output.
    pub fn invalidate_workspace_cache(&self) -> anyhow::Result<()> {
        match fs::remove_dir_all(&self.root) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| {
                format!(
                    "while attempting to remove package cache namespace {}",
                    self.root.display(),
                )
            }),
        }
    }

    fn write_artifact_bytes(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
        // Cache artifacts must appear atomically: readers either observe the previous complete
        // payload or the newly committed one, never a partially written file.
        let mut file = AtomicWriteFile::options().open(path).with_context(|| {
            format!(
                "while attempting to start atomic package cache write {}",
                path.display(),
            )
        })?;
        file.write_all(bytes).with_context(|| {
            format!(
                "while attempting to write package cache artifact {}",
                path.display(),
            )
        })?;
        file.commit().with_context(|| {
            format!(
                "while attempting to commit package cache artifact {}",
                path.display(),
            )
        })
    }

    fn packages_dir(&self) -> PathBuf {
        self.root.join(CACHE_PACKAGES_DIR_NAME)
    }

    fn generation_dir(&self) -> PathBuf {
        self.packages_dir().join(self.generation_dir_name())
    }

    fn generation_dir_name(&self) -> String {
        format!("{CACHE_GENERATION_DIR_PREFIX}{}", self.generation)
    }
}

/// Package artifact writer for a cache generation whose directory is already prepared.
pub(crate) struct PreparedPackageCacheWriter<'a> {
    store: &'a PackageCacheStore,
}

impl PreparedPackageCacheWriter<'_> {
    pub(crate) fn write_artifact(&self, artifact: &PackageCacheArtifact) -> anyhow::Result<()> {
        let bytes = PackageCacheCodec::encode_artifact(artifact)?;
        let path = self.store.package_artifact_path(&artifact.header.package);
        PackageCacheStore::write_artifact_bytes(&path, bytes.as_ref())
    }
}
