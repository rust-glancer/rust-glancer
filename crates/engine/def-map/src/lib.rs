mod build;
mod macro_expansion;
mod profile;
mod query;
mod store;
#[doc(hidden)]
pub mod testonly;

pub use rg_workspace::PackageSlot;

pub use self::{
    build::DefMapPerformancePreference,
    macro_expansion::BodyMacroExpander,
    profile::profile_descriptors,
    query::{DefMapCursorCandidate, DefMapPathCompletionSite, DefMapUnqualifiedCompletionSite},
    store::{DefMapDb, DefMapReadTxn, DefMapStats},
};

#[cfg(test)]
mod tests;
