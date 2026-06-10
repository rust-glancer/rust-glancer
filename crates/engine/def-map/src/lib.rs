mod build;
mod query;
mod store;
#[doc(hidden)]
pub mod testonly;

pub use rg_workspace::PackageSlot;

pub use self::{
    build::{DefMapFinalizationStats, DefMapPerformancePreference},
    query::{DefMapCursorCandidate, DefMapPathCompletionSite, DefMapUnqualifiedCompletionSite},
    store::{DefMapDb, DefMapReadTxn, DefMapStats},
};

#[cfg(test)]
mod tests;
