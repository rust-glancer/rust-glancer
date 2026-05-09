//! Package cache artifact model.
//!
//! Project-level code owns invalidation because only it can see Cargo metadata, workspace graph
//! changes, and the selected residency policy. Lower storage layers should receive already-vetted
//! artifact handles and package payloads.
//
// Dev note: At least for the time being, we avoid introducing DTO types for everything declared
// in the actual engine. It's dozens of types, and the serialization layer is a cache that is
// fine to throw away, e.g. we don't expect stability here. Adding DTO layer would result in
// having to basically copy-paste a ton of types with no clear benefit. At the same time, at least
// currently we don't plan to use `rkyv` archives, so it's merely a serialization layer, so
// (de)serialization quirks spreading throuhgout the codebase is not really a concern.

mod cached;
mod codec;
mod fingerprint;
mod header;
mod payload;
mod plan;
mod store;

pub use self::{
    cached::{
        CachedDependency, CachedPackage, CachedPackageId, CachedPackageSlot, CachedPackageSource,
        CachedPath, CachedRustEdition, CachedTarget, CachedTargetKind,
    },
    codec::PackageCacheCodec,
    fingerprint::Fingerprint,
    header::{CURRENT_PACKAGE_CACHE_SCHEMA_VERSION, PackageCacheHeader, PackageCacheSchemaVersion},
    payload::{PackageCacheArtifact, PackageCacheBodyIrState, PackageCachePayload},
    plan::WorkspaceCachePlan,
    store::PackageCacheStore,
};

#[cfg(test)]
mod tests;
