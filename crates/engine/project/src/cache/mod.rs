//! Disposable package cache for retained analysis state.
//!
//! Project loading owns cache policy because it can see the workspace graph, saved source
//! fingerprints, and residency configuration. This module owns the durable representation and the
//! mechanics needed to publish and read it safely. A cache hit is an optimization; any missing,
//! stale, or malformed artifact is rejected and rebuilt rather than partially salvaged.
//!
//! Each workspace graph selects one generation directory. Inside it, every package is one atomic
//! `.rgpkg` file with this coarse layout:
//!
//! ```text
//! fixed directory | probe | DefMap | Semantic IR | Body IR
//! ```
//!
//! The probe contains the package header, frozen parse snapshot, and Body IR coverage. Startup
//! reads only the fixed directory and probe to validate source identity. A query that touches an
//! offloaded package opens one immutable artifact revision and decodes only the required phase.
//! Body IR has its own nested directory because it is normally read one source file at a time.
//!
//! Individual package files are replaced atomically. A package-set update also leaves an
//! `update-in-progress` marker until every artifact is committed. If the process stops halfway,
//! the next startup throws the whole package set away; preserving a mixed cache is both less safe
//! and more complicated than rebuilding it.
//
// Dev note: At least for the time being, we avoid introducing DTO types for everything declared
// in the actual engine. It's dozens of types, and the serialization layer is a cache that is
// fine to throw away, e.g. we don't expect stability here. Adding DTO layer would result in
// having to basically copy-paste a ton of types with no clear benefit. At the same time, at least
// currently we use wincode as a serialization layer rather than as a stable cache contract, so
// (de)serialization quirks spreading throughout the codebase is not really a concern.

mod cached;
mod codec;
mod fingerprint;
mod header;
mod instance;
mod payload;
mod plan;
mod store;

pub use self::{
    cached::{
        CachedCfgOptions, CachedDependency, CachedPackage, CachedPackageId, CachedPackageSlot,
        CachedPackageSource, CachedPath, CachedRustEdition, CachedTarget, CachedTargetKind,
    },
    codec::PackageCacheCodec,
    fingerprint::Fingerprint,
    header::{CURRENT_PACKAGE_CACHE_SCHEMA_VERSION, PackageCacheHeader},
    payload::{PackageCacheArtifact, PackageCachePayload, PackageCacheProbe},
    plan::WorkspaceCachePlan,
    store::PackageCacheStore,
};

pub(crate) use self::{
    instance::PackageCacheInstance,
    store::{PackageArtifactReader, PackageCacheUpdate},
};

#[cfg(test)]
mod tests;
