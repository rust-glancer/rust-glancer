//! Resident Body IR packages and their cache-facing storage units.
//!
//! Query code normally sees [`PackageBodies`] and [`CrateBodies`]: compact arenas indexed by the
//! engine's stable ids. The cache cannot decode one source file out of those arenas directly, so
//! [`shard`] describes a second representation made of a small routing manifest and one payload
//! per source file. Loading every shard reconstructs exactly the ordinary resident representation.

mod body_local_items;
mod coverage;
mod resident;
mod shard;

pub use self::{
    body_local_items::BodyLocalItems,
    coverage::{CrateBodiesCoverage, CrateBodiesStatus, PackageBodiesCoverage},
    resident::{CrateBodies, PackageBodies},
    shard::{BodyFileEntry, BodyFileShard, CrateBodiesManifest, PackageBodiesManifest},
};
