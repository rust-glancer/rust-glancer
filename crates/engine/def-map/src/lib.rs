mod build;
mod model;
mod query;
mod store;

pub use rg_workspace::PackageSlot;

pub use self::{
    model::{
        DefId, DefMap, ImportBinding, ImportData, ImportId, ImportKind, ImportPath, ImportRef,
        ImportSourcePath, LocalDefData, LocalDefId, LocalDefKind, LocalDefRef, LocalImplData,
        LocalImplId, LocalImplRef, ModuleData, ModuleId, ModuleOrigin, ModuleRef, ModuleScope,
        Package, Path, PathSegment, ScopeBinding, ScopeEntry, TargetRef,
    },
    query::{DefMapCursorCandidate, ResolvePathResult},
    store::{DefMapDb, DefMapReadTxn, DefMapStats},
};

#[cfg(test)]
mod tests;
