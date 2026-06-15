//! Per-process cache namespace ownership.
//!
//! Each live LSP engine claims one numbered instance directory and keeps its lock file held for
//! the whole project lifetime. That makes package artifacts private to the engine that may later
//! lazy-load them.

use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Context as _;
use rg_workspace::WorkspaceMetadata;

const CACHE_DIR_NAME: &str = "rust_glancer";
const CACHE_INSTANCES_DIR_NAME: &str = "instances";
const CACHE_INSTANCE_LOCK_FILE_NAME: &str = "instance.lock";
const MAX_CACHE_INSTANCE_SLOTS: u64 = 1024;

/// Owned cache namespace for one live project/LSP engine.
#[derive(Debug, Clone)]
pub(crate) struct PackageCacheInstance {
    inner: Arc<PackageCacheInstanceInner>,
}

/// Shared inner state keeps the OS lock alive across cloned cache state handles.
#[derive(Debug)]
struct PackageCacheInstanceInner {
    root: PathBuf,
    #[cfg(test)]
    slot: u64,
    _lock_file: fs::File,
}

impl PackageCacheInstance {
    /// Claim the first available cache instance under Cargo's target directory.
    pub(crate) fn for_workspace(workspace: &WorkspaceMetadata) -> anyhow::Result<Self> {
        let target_dir = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace.workspace_root().join("target"));

        let instances_root = Self::instances_root(workspace, target_dir);
        for slot in 1..=MAX_CACHE_INSTANCE_SLOTS {
            if let Some(inner) = Self::try_claim_slot(&instances_root, slot)? {
                return Ok(Self {
                    inner: Arc::new(inner),
                });
            }
        }

        anyhow::bail!(
            "no free package cache instance slots under {}",
            instances_root.display()
        )
    }

    /// Return this engine's private cache root.
    pub(crate) fn root(&self) -> &Path {
        &self.inner.root
    }

    /// Return the selected slot number for direct ownership tests.
    #[cfg(test)]
    pub(crate) fn slot_for_tests(&self) -> u64 {
        self.inner.slot
    }

    fn try_claim_slot(
        instances_root: &Path,
        slot: u64,
    ) -> anyhow::Result<Option<PackageCacheInstanceInner>> {
        let root = instances_root.join(slot.to_string());
        fs::create_dir_all(&root).with_context(|| {
            format!(
                "while attempting to create package cache instance {}",
                root.display(),
            )
        })?;

        let lock_path = root.join(CACHE_INSTANCE_LOCK_FILE_NAME);
        let lock_file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| {
                format!(
                    "while attempting to open package cache instance lock {}",
                    lock_path.display(),
                )
            })?;

        match lock_file.try_lock() {
            Ok(()) => Ok(Some(PackageCacheInstanceInner {
                root,
                #[cfg(test)]
                slot,
                _lock_file: lock_file,
            })),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(error)) => Err(error).with_context(|| {
                format!(
                    "while attempting to lock package cache instance {}",
                    lock_path.display(),
                )
            }),
        }
    }

    fn instances_root(workspace: &WorkspaceMetadata, target_dir: impl Into<PathBuf>) -> PathBuf {
        let workspace_name = workspace
            .workspace_root()
            .file_name()
            .unwrap_or_else(|| OsStr::new("workspace"));

        target_dir
            .into()
            .join(CACHE_DIR_NAME)
            .join(workspace_name)
            .join(CACHE_INSTANCES_DIR_NAME)
    }
}
