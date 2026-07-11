//! Cache paths and atomic package-set updates.
//!
//! A generation directory belongs to one workspace graph. Individual package files are replaced
//! atomically, while an update marker extends that guarantee across the package set: an interrupted
//! update is discarded on the next startup instead of exposing mutually inconsistent artifacts.
//!
//! The on-disk shape under one claimed cache instance is:
//!
//! ```text
//! packages/
//!   graph-<workspace fingerprint>/
//!     update-in-progress
//!     package-<slot>-<name>-<package fingerprint>.rgpkg
//! ```
//!
//! The marker is written before the first change and removed only after a successful commit. It is
//! intentionally coarse: if the process stops after replacing three out of ten artifacts, startup
//! removes the disposable package cache instead of trying to determine which cross-package ids
//! still agree.

use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use anyhow::Context as _;
use atomic_write_file::AtomicWriteFile;
use rg_workspace::WorkspaceMetadata;

use super::super::{
    CachedPackage, Fingerprint, PackageCacheCodec, PackageCacheInstance, PackageCacheWriteInput,
    WorkspaceCachePlan,
};

const CACHE_PACKAGES_DIR_NAME: &str = "packages";
const CACHE_GENERATION_DIR_PREFIX: &str = "graph-";
const PACKAGE_ARTIFACT_EXTENSION: &str = "rgpkg";
const CACHE_UPDATE_MARKER_FILE_NAME: &str = "update-in-progress";

/// Paths for one cache instance and one workspace-graph generation.
///
/// `root` is the already-claimed instance directory. `generation` selects the subdirectory for the
/// workspace graph. `workspace_root` is kept so package fingerprints can normalize workspace-local
/// paths the same way when constructing and looking up an artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageCacheStore {
    workspace_root: PathBuf,
    root: PathBuf,
    generation: Fingerprint,
}

impl PackageCacheStore {
    /// Bind a workspace cache plan to the instance directory claimed by this engine process.
    ///
    /// This does not touch the filesystem. Directory creation is delayed until an update begins,
    /// while cache reads can simply observe a missing artifact as a cache miss.
    pub(crate) fn for_instance(
        workspace: &WorkspaceMetadata,
        cache_plan: &WorkspaceCachePlan,
        instance: &PackageCacheInstance,
    ) -> Self {
        Self {
            workspace_root: workspace.workspace_root().to_path_buf(),
            root: instance.root().to_path_buf(),
            generation: cache_plan.fingerprint(workspace.workspace_root()),
        }
    }

    /// Expose the selected cache root to cache layout tests.
    #[cfg(test)]
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Return the complete path for one package identity in this graph generation.
    ///
    /// The slot and name make paths readable; the package fingerprint prevents two different Cargo
    /// package descriptions that occupy the same slot from sharing bytes accidentally.
    pub fn package_artifact_path(&self, package: &CachedPackage) -> PathBuf {
        let fingerprint = self.package_fingerprint(package);
        let file_name = format!(
            "package-{}-{}-{}.{}",
            package.package.0, package.name, fingerprint, PACKAGE_ARTIFACT_EXTENSION,
        );

        self.generation_dir().join(file_name)
    }

    /// Fingerprint package metadata with paths interpreted relative to this workspace root.
    pub fn package_fingerprint(&self, package: &CachedPackage) -> Fingerprint {
        package.fingerprint(&self.workspace_root)
    }

    /// Removes cache data that cannot be reached through the current workspace graph generation.
    ///
    /// The store deliberately does not track individual artifacts. A source-only save rewrites the
    /// affected package files inside the same generation directory, while Cargo graph changes pick
    /// a new generation and make the older directories disposable.
    pub(crate) fn cleanup_stale_generations(&self) -> anyhow::Result<()> {
        // A cache instance may be new or already empty. There is nothing to clean in either case.
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

        // Only directories created by this generation naming scheme are disposable here. Leave
        // unrelated files alone, as well as the generation selected by the current workspace plan.
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

    /// Start a package-set update and publish its incomplete marker before any package write.
    ///
    /// The returned guard does not remove the marker on drop. The caller must write every intended
    /// artifact and call [`PackageCacheUpdate::commit`]. Any early return leaves evidence that the
    /// next startup should discard the mixed package set.
    pub(crate) fn begin_artifact_update(&self) -> anyhow::Result<PackageCacheUpdate<'_>> {
        let package_dir = self.generation_dir();
        fs::create_dir_all(&package_dir).with_context(|| {
            format!(
                "while attempting to create package cache directory {}",
                package_dir.display(),
            )
        })?;

        let marker = self.cache_update_marker_path();
        if marker.try_exists().with_context(|| {
            format!(
                "while attempting to inspect package cache update marker {}",
                marker.display(),
            )
        })? {
            // TODO(#128): Recover failed cache transactions in-process by rebuilding coherent
            // backing for every offloaded package, rather than requiring an engine restart.
            anyhow::bail!(
                "package cache update marker already exists at {}",
                marker.display(),
            );
        }

        // The package set is one coarse cache transaction. If the process stops after replacing
        // only some artifacts, startup sees this marker and discards the whole disposable cache
        // instead of trying to infer which cross-package IDs still agree.
        Self::write_atomically(&marker, |file| {
            file.write_all(b"package cache update in progress\n")
        })?;

        Ok(PackageCacheUpdate { store: self })
    }

    /// Discard package artifacts left by an interrupted package-set update.
    ///
    /// Removing the whole `packages` directory removes the marker as well. This is deliberately
    /// simpler than attempting to resume or roll back individual package replacements.
    pub(crate) fn recover_incomplete_update(&self) -> anyhow::Result<()> {
        let marker = self.cache_update_marker_path();
        if !marker.try_exists().with_context(|| {
            format!(
                "while attempting to inspect package cache update marker {}",
                marker.display(),
            )
        })? {
            return Ok(());
        }

        self.clear_package_artifacts().with_context(|| {
            format!(
                "while attempting to discard incomplete package cache update {}",
                marker.display(),
            )
        })?;
        Ok(())
    }

    /// Removes package artifacts from this cache instance.
    ///
    /// The instance root also contains the live ownership lock, so invalidation only clears the
    /// disposable package data below it.
    pub(crate) fn clear_package_artifacts(&self) -> anyhow::Result<()> {
        let packages_dir = self.packages_dir();
        match fs::remove_dir_all(&packages_dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| {
                format!(
                    "while attempting to remove package cache artifacts {}",
                    packages_dir.display(),
                )
            }),
        }
    }

    /// Replace one complete file without exposing a partially written payload.
    fn write_atomically(
        path: &Path,
        write: impl FnOnce(&mut AtomicWriteFile) -> std::io::Result<()>,
    ) -> anyhow::Result<()> {
        // Cache artifacts must appear atomically: readers either observe the previous complete
        // payload or the newly committed one, never a partially written file.
        let mut file = AtomicWriteFile::options().open(path).with_context(|| {
            format!(
                "while attempting to start atomic package cache write {}",
                path.display(),
            )
        })?;
        write(&mut file).with_context(|| {
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

    fn cache_update_marker_path(&self) -> PathBuf {
        self.generation_dir().join(CACHE_UPDATE_MARKER_FILE_NAME)
    }
}

/// One package-set cache update whose incomplete marker survives unless explicitly committed.
///
/// Dropping this value after a write failure deliberately leaves the marker in place. The next
/// startup then discards every package artifact rather than attempting to salvage a mixed set.
pub(crate) struct PackageCacheUpdate<'a> {
    store: &'a PackageCacheStore,
}

impl PackageCacheUpdate<'_> {
    /// Encode and atomically replace one artifact inside this still-incomplete package set.
    ///
    /// Success here is not a package-set commit. The marker remains until the owner has written all
    /// affected packages and calls `commit`.
    /// Encode borrowed resident phases and write their final fragments in format order.
    pub(crate) fn write_input(&self, input: PackageCacheWriteInput<'_>) -> anyhow::Result<()> {
        let encoded = PackageCacheCodec::encode_write_input(input)?;
        let path = self.store.package_artifact_path(&input.header.package);
        PackageCacheStore::write_atomically(&path, |file| encoded.write_to(file))
    }

    /// Commit the package set by removing the marker only after every artifact is durable.
    ///
    /// Marker removal is the final step. A startup that sees no marker may therefore trust that it
    /// is not observing a package set abandoned halfway through an update.
    pub(crate) fn commit(self) -> anyhow::Result<()> {
        let marker = self.store.cache_update_marker_path();
        fs::remove_file(&marker).with_context(|| {
            format!(
                "while attempting to commit package cache update {}",
                marker.display(),
            )
        })
    }
}
