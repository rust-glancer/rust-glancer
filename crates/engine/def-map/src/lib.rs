mod build;
mod model;
mod query;
mod store;

pub use rg_workspace::PackageSlot;

pub use self::{
    build::DefMapFinalizationStats,
    def_map::DefMap,
    model::{
        GeneratedItemRef, GeneratedSourceData, GeneratedSourceId, ImportBinding, ImportData,
        ImportKind, ImportPath, ImportSourcePath, ItemSource, ItemSourceKind, LocalDefData,
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

pub(crate) use self::model::{
    DefId, ImportId, LocalDefId, LocalDefRef, LocalImplId, LocalImplRef, ModuleId, ModuleRef,
    TargetRef,
};

mod def_map;

#[cfg(test)]
mod tests;
