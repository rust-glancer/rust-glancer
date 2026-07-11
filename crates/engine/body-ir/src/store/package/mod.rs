//! Resident Body IR packages and their cache-facing storage units.
//!
//! Query code normally sees [`PackageBodies`] and [`TargetBodies`]: compact arenas indexed by the
//! engine's stable ids. The cache cannot decode one source file out of those arenas directly, so
//! [`shard`] describes a second representation made of a small routing manifest, one target-global
//! semantic index, and one payload per source file. Loading every shard reconstructs exactly the
//! ordinary resident representation.

mod coverage;
mod resident;
mod shard;

pub use self::{
    coverage::{TargetBodiesCoverage, TargetBodiesStatus},
    resident::{PackageBodies, TargetBodies},
    shard::{BodyFileEntry, BodyFileShard, PackageBodiesManifest, TargetBodiesManifest},
};
