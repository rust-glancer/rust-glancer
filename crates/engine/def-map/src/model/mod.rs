//! Def-map domain model.

pub(crate) mod data;
pub(crate) mod import;
pub(crate) mod package;
pub(crate) mod path;
pub(crate) mod scope;
pub(crate) mod visible_scopes;

pub use self::{
    data::{
        LocalDefData, LocalDefKind, LocalImplData, MacroDefinitionData, MacroDefinitionPayload,
        ModuleData, ModuleOrigin,
    },
    import::{ImportBinding, ImportData, ImportKind, ImportPath, ImportSourcePath},
    package::PackageDefMaps,
    path::{Path, PathSegment},
    scope::{
        ModuleScope, ModuleScopeBuilder, Namespace, ScopeBinding, ScopeBindingOrigin, ScopeEntry,
    },
    visible_scopes::{ScopeNamespace, VisibleScopeDef, VisibleScopeDefs, VisibleScopeOrigin},
};

pub(crate) use self::scope::ScopeEntryRef;
