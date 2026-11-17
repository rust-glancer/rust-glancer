//! Builds saved analysis eagerly and completes policy-excluded targets on request.

pub mod bench_support;
mod bodies;
mod builder;
mod cache_probe;
mod checkpoint_memory;
mod config;
mod initial;
mod macro_source_files;
mod phases;
mod plan;
mod rebuild;
mod recovery;

pub use self::{builder::ProjectBuilder, config::StartupCacheLoad};
pub(crate) use self::{rebuild::rebuild_packages, recovery::recover_from_cache_load_failure};
