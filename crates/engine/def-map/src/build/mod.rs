//! Builds frozen def-map snapshots by replacing selected packages in a shaped baseline.
//!
//! Collection records direct declarations and raw imports before names from other modules and
//! crates have been resolved. Selected packages receive fresh mutable `CrateBuilder`s;
//! resolution reads every other package from the frozen baseline.
//!
//! Macro expansion can reveal real files after the initial Parse/ItemTree pass. For example,
//! expanded syntax may contain `mod generated;` or an
//! `include!(concat!(env!("OUT_DIR"), "/bindings.rs"))`. DefMap knows how those constructs affect
//! modules and scopes, but source capture belongs to the project layer.
//! `DefMapDb::build_packages` collects selected package state once and calls an injected source
//! loader between expansion batches until the fixed point can be frozen.

mod collect;
mod crates;
mod implicit_roots;
mod imports;
mod macro_source_files;
mod macros;
mod output;
mod packages;
mod resolution_env;
mod scopes;

pub(crate) use self::macro_source_files::MacroSourceFileResolutions;
pub use self::{
    imports::{
        ImportCollectionEnv, ImportCollectionOutput, ImportCollector, ModuleImportInput,
        ResolvedModuleImports,
    },
    macro_source_files::{
        MacroSourceFileLoader, MacroSourceFileRequest, MacroSourceFileResolution,
    },
    output::{DefMapBuildOutput, GeneratedItemStores},
};
