//! Builds frozen def-map snapshots by replacing selected packages in a shaped baseline.
//!
//! Crate collection intentionally stops before cross-crate facts such as implicit roots,
//! preludes, and imports are fully known. Selected packages receive fresh mutable `CrateState`s;
//! resolution reads every other package from the frozen baseline.
//!
//! Construction is resumable because macro expansion can reveal out-of-line modules whose source
//! belongs to the project layer. `DefMapDb::start_package_build` collects selected package state
//! once, then the project alternates source discovery with session advances until the fixed point
//! can be frozen.

mod collect;
mod finalize;
mod generated_modules;
mod implicit_roots;
mod imports;
mod macros;
mod session;

pub(crate) use self::generated_modules::{GeneratedModuleResolution, GeneratedModuleResolutions};
pub use self::{
    generated_modules::{DefMapBuildProgress, GeneratedModuleRequest},
    session::DefMapBuildSession,
};
