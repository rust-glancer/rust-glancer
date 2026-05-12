//! Versioned package artifact headers.
//!
//! The header is the first data read from an artifact. It keeps the schema version next to the
//! cached package metadata so stale or mismatched files can be rejected before loading analysis
//! payloads.

use wincode::{SchemaRead, SchemaWrite};

use super::{cached::CachedPackage, fingerprint::Fingerprint};

/// Current on-disk package artifact schema.
pub const CURRENT_PACKAGE_CACHE_SCHEMA_VERSION: PackageCacheSchemaVersion =
    PackageCacheSchemaVersion(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite)]
pub struct PackageCacheSchemaVersion(pub u32);

/// Header shared by future package cache artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite)]
pub struct PackageCacheHeader {
    pub schema_version: PackageCacheSchemaVersion,
    pub package: CachedPackage,
    pub source_fingerprint: Fingerprint,
}

impl PackageCacheHeader {
    pub fn new(package: CachedPackage, source_fingerprint: Fingerprint) -> Self {
        Self {
            schema_version: CURRENT_PACKAGE_CACHE_SCHEMA_VERSION,
            package,
            source_fingerprint,
        }
    }
}
