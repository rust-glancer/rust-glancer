//! Produces saved analysis from source and completes missing body coverage.
//!
//! Initial construction and package rebuilds run the same semantic phases with different input
//! sets. Batch indexing bounds how many packages are in flight; split indexing lets body analysis
//! finish after declarations are available. These choices have separate scheduling and publication
//! rules, while sharing package selection, source discovery, and storage.

mod batch;
pub mod bench_support;
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
mod split;

pub use self::{
    builder::ProjectBuilder,
    config::{
        IndexingPerformancePreference, PackageBatchSize, SplitIndexingMode, StartupCacheLoad,
    },
    split::{
        AnalysisSurface, BodyPublication, BodyPublicationOutcome, SavedBodyBuildInputs,
        SavedBodyProducts, SplitIndexing, SplitIndexingProgress, SplitIndexingStage,
    },
};
pub(crate) use self::{rebuild::rebuild_packages, recovery::recover_from_cache_load_failure};
