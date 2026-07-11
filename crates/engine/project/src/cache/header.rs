//! Versioned package artifact headers.
//!
//! The header lives inside the small probe, immediately after the fixed container directory. It
//! keeps schema, package identity, and saved-source identity together so stale or mismatched files
//! are rejected before any retained IR is loaded.

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
    PackageCacheSchemaVersion(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
#[memsize(leaf)]
pub struct PackageCacheSchemaVersion(pub u32);

/// Identity and compatibility information for one package artifact revision.
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
