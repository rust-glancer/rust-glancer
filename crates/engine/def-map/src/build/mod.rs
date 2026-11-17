//! Collect and resolve one configured target at a time.

mod collect;
mod crates;
mod fixture;
mod imports;
mod macro_source_files;
mod macros;
mod output;
mod plan;
mod resolution_env;
mod resolve;
mod target;

pub(crate) use self::{
    fixture::build_fixture_definitions,
    macro_source_files::{
        MacroSourceFileRequest, MacroSourceFileResolution, MacroSourceFileResolutions,
    },
};
pub use self::{
    imports::{
        ImportCollectionEnv, ImportCollectionOutput, ImportCollector, ModuleImportInput,
        ResolvedModuleImports,
    },
    output::CrateBuildOutput,
    target::CrateDependencies,
};
