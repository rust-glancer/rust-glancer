//! Typed read failures at the cache/package-store boundary.
//!
//! Filesystem operations and cache validation need different diagnostics. An I/O error keeps its
//! path and operating-system source. A malformed error keeps the cache-specific reason, such as a
//! header mismatch or invalid byte range. The project can then apply its disposable-cache policy
//! without erasing the reason a package could not be loaded.

use std::{fmt, path::PathBuf};

use rg_package_store::{MalformedCacheError, PackageStoreError};

/// Failure from opening, reading, or decoding one package artifact revision.
#[derive(Debug)]
pub(crate) enum PackageCacheReadError {
    /// The filesystem could not provide bytes that were requested normally.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Bytes were present, but they did not satisfy the cache format or expected identity.
    Malformed { source: MalformedCacheError },
}

impl PackageCacheReadError {
    /// Attach the logical package slot expected by the generic package-store API.
    ///
    /// The cache reader knows paths and artifact identities; query code knows package slots. This
    /// conversion preserves both layers in the final error without making `rg_package_store`
    /// depend on the project cache format.
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
