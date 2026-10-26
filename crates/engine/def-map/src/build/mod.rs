//! Builds frozen def-map snapshots by replacing selected packages in a shaped baseline.
//!
//! Crate collection intentionally stops before cross-crate facts such as implicit roots,
//! preludes, and imports are fully known. Selected packages receive fresh mutable `CrateState`s;
//! resolution reads every other package from the frozen baseline.
//!
//! Macro expansion can reveal real files after the initial Parse/ItemTree pass. For example,
//! expanded syntax may contain `mod generated;` or an
//! `include!(concat!(env!("OUT_DIR"), "/bindings.rs"))`. DefMap knows how those constructs affect
//! modules and scopes, but source capture belongs to the project layer.
//! `DefMapDb::build_packages` collects selected package state once and calls an injected source
//! loader between expansion batches until the fixed point can be frozen.

mod collect;
mod finalize;
mod implicit_roots;
mod imports;
mod macro_source_files;
mod macros;
mod output;
mod packages;

pub(crate) use self::macro_source_files::MacroSourceFileResolutions;
pub use self::{
    macro_source_files::{
        MacroSourceFileLoader, MacroSourceFileRequest, MacroSourceFileResolution,
    },
    output::{DefMapBuildOutput, GeneratedItemStores},
};
