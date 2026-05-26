mod build;
mod model;
mod query;
mod store;

pub use rg_workspace::PackageSlot;

pub use self::{
    build::DefMapFinalizationStats,
    model::{
        DefMap, GeneratedItemRef, GeneratedSourceData, GeneratedSourceId, ImportBinding,
        ImportData, ImportKind, ImportPath, ImportSourcePath, ItemSource, ItemSourceKind,
        LocalDefData, LocalDefKind, LocalImplData, MacroDefinitionData, MacroDefinitionPayload,
        ModuleData, ModuleOrigin, ModuleScope, Package, Path, PathSegment, ScopeBinding,
        ScopeBindingOrigin, ScopeEntry,
    },
    query::{
        DefMapCursorCandidate, DefMapPathCompletionSite, DefMapUnqualifiedCompletionSite,
        ResolvePathResult, ScopeNamespace, VisibleScopeDef, VisibleScopeOrigin,
    },
    store::{DefMapDb, DefMapReadTxn, DefMapStats},
};

pub(crate) use self::model::{
    DefId, ImportId, ImportRef, LocalDefId, LocalDefRef, LocalImplId, LocalImplRef, ModuleId,
    ModuleRef, TargetRef,
};

#[cfg(test)]
mod tests;
