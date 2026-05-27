//! Def-map domain model.

pub(crate) mod data;
pub(crate) mod import;
pub(crate) mod package;
pub(crate) mod path;
pub(crate) mod scope;
pub(crate) mod source;

pub use self::{
    data::{
        LocalDefData, LocalDefKind, LocalImplData, MacroDefinitionData, MacroDefinitionPayload,
        ModuleData, ModuleOrigin,
    },
    import::{ImportBinding, ImportData, ImportKind, ImportPath, ImportSourcePath},
    package::PackageDefMaps,
    path::{Path, PathSegment},
    scope::{ModuleScope, ScopeBinding, ScopeBindingOrigin, ScopeEntry},
    source::{
        GeneratedItemRef, GeneratedSourceData, GeneratedSourceId, ItemSource, ItemSourceKind,
    },
};

pub(crate) use self::scope::{ModuleScopeBuilder, Namespace, ScopeEntryRef};
