mod build;
mod model;
mod query;
mod store;

pub use rg_workspace::PackageSlot;

pub use self::{
    build::DefMapFinalizationStats,
    model::{
        DefId, DefMap, GeneratedItemId, GeneratedItemRef, ImportBinding, ImportData, ImportId,
        ImportKind, ImportPath, ImportRef, ImportSourcePath, ItemSource, LocalDefData, LocalDefId,
        LocalDefKind, LocalDefRef, LocalImplData, LocalImplId, LocalImplRef, MacroDefinitionData,
        MacroDefinitionPayload, ModuleData, ModuleId, ModuleOrigin, ModuleRef, ModuleScope,
        Package, Path, PathSegment, ScopeBinding, ScopeBindingOrigin, ScopeEntry, TargetRef,
    },
    query::{
        DefMapCursorCandidate, DefMapPathCompletionSite, DefMapUnqualifiedCompletionSite,
        ResolvePathResult, ScopeNamespace, VisibleScopeDef, VisibleScopeOrigin,
    },
    store::{DefMapDb, DefMapReadTxn, DefMapStats},
};

#[cfg(test)]
mod tests;
