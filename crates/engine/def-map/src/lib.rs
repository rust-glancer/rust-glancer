mod build;
mod model;
mod query;
mod store;

pub use rg_workspace::PackageSlot;

pub use self::{
    build::DefMapFinalizationStats,
    def_map::DefMap,
    model::{
        ImportBinding, ImportData, ImportKind, ImportPath, ImportSourcePath, LocalDefData,
        LocalDefKind, LocalImplData, MacroDefinitionData, MacroDefinitionPayload, ModuleData,
        ModuleOrigin, ModuleScope, PackageDefMaps, Path, PathSegment, ScopeBinding,
        ScopeBindingOrigin, ScopeEntry,
    },
    query::{
        DefMapCursorCandidate, DefMapPathCompletionSite, DefMapUnqualifiedCompletionSite,
        ResolvePathResult, ScopeNamespace, VisibleScopeDef, VisibleScopeOrigin,
    },
    store::{DefMapDb, DefMapReadTxn, DefMapStats},
};

mod def_map;

#[cfg(test)]
mod tests;
