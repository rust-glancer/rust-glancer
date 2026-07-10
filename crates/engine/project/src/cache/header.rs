//! Versioned package artifact headers.
//!
//! The header is the first data read from an artifact. It keeps the schema version next to the
//! cached package metadata so stale or mismatched files can be rejected before loading analysis
//! payloads.

use rg_std::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

use super::{cached::CachedPackage, fingerprint::Fingerprint};

/// Current on-disk package artifact schema.
//
// TODO (low priority): Maaaybe include an explicit analysis-producer epoch in cache validity. The schema
// version proves that bytes are decodable, but semantic algorithm changes can invalidate an otherwise
// unchanged payload. While the Cargo package version alone is insufficient for developer builds that remain
// 0.1.0, it doesn't matter in practice: devs know what's going on and can either remove cache when needed or
// save file to reindex relevant chunk. Devs are no babies! Thus, no need to overcomplicate architecture for
// virtually no gain.
pub const CURRENT_PACKAGE_CACHE_SCHEMA_VERSION: PackageCacheSchemaVersion =
    PackageCacheSchemaVersion(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
#[memsize(leaf)]
pub struct PackageCacheSchemaVersion(pub u32);

/// Header shared by future package cache artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
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
