mod build;
mod query;
mod store;

pub use rg_ir_storage::{
    DefMap, DefMapBuilder, ImportBinding, ImportData, ImportKind, ImportPath, ImportSourcePath,
    LocalDefData, LocalDefKind, LocalImplData, MacroDefinitionData, MacroDefinitionPayload,
    ModuleData, ModuleOrigin, ModuleScope, ModuleScopeBuilder, Namespace, PackageDefMaps, Path,
    PathSegment, ScopeBinding, ScopeBindingOrigin, ScopeEntry, ScopeEntryRef, ScopeNamespace,
    TargetData, VisibleScopeDef, VisibleScopeDefs, VisibleScopeOrigin,
};
pub use rg_workspace::PackageSlot;

pub use self::{
    build::DefMapFinalizationStats,
    query::{
        DefMapCursorCandidate, DefMapPathCompletionSite, DefMapQuery, DefMapSource,
        DefMapUnqualifiedCompletionSite, NameResolutionFilter, ResolvePathResult,
    },
    store::{DefMapDb, DefMapReadTxn, DefMapStats},
};

#[cfg(test)]
mod tests;
