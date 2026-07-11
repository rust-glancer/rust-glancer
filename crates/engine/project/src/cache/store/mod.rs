//! Filesystem access for package cache artifacts.
//!
//! There are two different jobs here. [`PackageCacheStore`] owns cache paths and atomic
//! package-set updates. [`PackageArtifactReader`] owns one already-open artifact revision and
//! serves independently decodable sections from it. Keeping those jobs separate matters because
//! a read transaction must never reopen a path midway through a request and accidentally combine
//! sections from two revisions.
//!
//! The usual read path is:
//!
//! ```text
//! PackageCacheStore
//!     -> open and validate fixed directory + probe
//!     -> PackageArtifactReader pinned to that file revision
//!     -> read DefMap, Semantic IR, or selected Body IR units
//! ```
//!
//! The write path stays in [`filesystem`]. It atomically replaces each package file and uses an
//! update marker around the complete package set. The read path stays in [`artifact`]. It owns file
//! cursors, byte-range checks, decoding state, and conversion into package-store errors.

mod artifact;
mod filesystem;

pub use filesystem::PackageCacheStore;

pub(crate) use self::{artifact::PackageArtifactReader, filesystem::PackageCacheUpdate};
