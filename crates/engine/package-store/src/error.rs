use std::path::PathBuf;

use rg_workspace::PackageSlot;

/// Failure to read one logical package from package storage.
#[derive(Debug, thiserror::Error)]
pub enum PackageStoreError {
    #[error("package slot {slot:?} is missing from the store")]
    MissingSlot { slot: PackageSlot },
    #[error("package slot {slot:?} is outside this read transaction's package subset")]
    ExcludedSlot { slot: PackageSlot },
    #[error("offloaded package slot {slot:?} {source}")]
    Load {
        slot: PackageSlot,
        #[source]
        source: PackageLoadError,
    },
}

impl PackageStoreError {
    pub fn missing_package(slot: PackageSlot) -> Self {
        Self::Load {
            slot,
            source: PackageLoadError::MissingPackage,
        }
    }

    pub fn io(slot: PackageSlot, path: PathBuf, source: std::io::Error) -> Self {
        Self::Load {
            slot,
            source: PackageLoadError::Io { path, source },
        }
    }

    pub fn malformed_cache(slot: PackageSlot, source: MalformedCacheError) -> Self {
        Self::Load {
            slot,
            source: PackageLoadError::MalformedCache { source },
        }
    }

    pub fn stale_package(slot: PackageSlot, reason: impl Into<String>) -> Self {
        Self::Load {
            slot,
            source: PackageLoadError::StalePackage {
                reason: reason.into(),
            },
        }
    }
}

/// Failure reported by the backing package loader for an offloaded slot.
#[derive(Debug, thiserror::Error)]
pub enum PackageLoadError {
    #[error("is missing from backing storage")]
    MissingPackage,
    #[error("could not be read from backing storage at {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("has malformed cache data: {source}")]
    MalformedCache {
        #[source]
        source: MalformedCacheError,
    },
    #[error("is stale: {reason}")]
    StalePackage { reason: String },
}

/// Cache artifact contents that were readable but cannot be trusted as a package payload.
#[derive(Debug, thiserror::Error)]
pub enum MalformedCacheError {
    #[error("failed to decode artifact {}: {reason}", path.display())]
    Decode { path: PathBuf, reason: String },
    #[error(
        "artifact {} belongs to package #{} `{}`, expected package #{} `{}`",
        path.display(),
        actual_slot,
        actual_name,
        expected_slot,
        expected_name,
    )]
    HeaderMismatch {
        path: PathBuf,
        actual_slot: u64,
        actual_name: String,
        expected_slot: u64,
        expected_name: String,
    },
    #[error("invalid artifact payload: {reason}")]
    InvalidPayload { reason: String },
}
