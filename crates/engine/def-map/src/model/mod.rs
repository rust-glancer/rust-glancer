//! Def-map domain model.

pub(crate) mod data;
pub(crate) mod ids;
pub(crate) mod import;
pub(crate) mod package;
pub(crate) mod path;
pub(crate) mod scope;

pub use self::{
    data::{
        DefMap, LocalDefData, LocalDefKind, LocalImplData, MacroDefinitionData,
        MacroDefinitionPayload, ModuleData, ModuleOrigin,
    },
    ids::{
        DefId, ImportId, ImportRef, LocalDefId, LocalDefRef, LocalImplId, LocalImplRef, ModuleId,
        ModuleRef, TargetRef,
    },
    import::{ImportBinding, ImportData, ImportKind, ImportPath, ImportSourcePath},
    package::Package,
    path::{Path, PathSegment},
    scope::{ModuleScope, ScopeBinding, ScopeEntry},
};

pub(crate) use self::{
    ids::ResidentTargetRef,
    import::ImportSourcePathSegment,
    scope::{ModuleScopeBuilder, Namespace, ScopeEntryRef, ScopeNameEntry},
};
