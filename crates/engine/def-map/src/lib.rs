mod build;
mod macro_expansion;
mod profile;
mod query;
mod store;
#[doc(hidden)]
pub mod testonly;

pub use rg_workspace::PackageSlot;

pub use rg_macro_runtime::MacroExpansionPerformancePreference;

pub use self::{
    macro_expansion::{
        BodyMacroCallOrigin, BodyMacroCallSite, BodyMacroExpander, BodyMacroExprExpansion,
        ExpandedBodyMacro,
    },
    profile::profile_descriptors,
    query::{DefMapCursorCandidate, DefMapPathCompletionSite, DefMapUnqualifiedCompletionSite},
    store::{DefMapDb, DefMapReadTxn, DefMapStats},
};

#[cfg(test)]
mod tests;
