mod build;
mod import;
mod local;
mod macro_expansion;
mod map;
mod module;
mod package;
mod profile;
mod query;
mod scope;
mod source;
mod store;
#[doc(hidden)]
pub mod testonly;
mod visible;

pub use rg_workspace::PackageSlot;

pub use rg_macro_runtime::MacroExpansionPerformancePreference;

pub use self::{
    build::{DefMapBuildProgress, DefMapRebuildSession, GeneratedModuleRequest},
    import::{ImportBinding, ImportData, ImportKind, ImportPath},
    local::{
        LocalDefData, LocalDefKind, LocalEnumVariantData, LocalEnumVariantEntry, LocalImplData,
        MacroDefinitionData, MacroDefinitionKind, MacroDefinitionPayload, MacroDefinitionView,
    },
    macro_expansion::{
        BodyMacroCallOrigin, BodyMacroCallSite, BodyMacroExpander, BodyMacroExpansionOutcome,
        BodyMacroExprExpansion, BodyMacroExprExpansionOutcome, BodyMacroPatExpansionOutcome,
        BodyMacroStmtExpansionOutcome, BodyMacroTypeExpansionOutcome, ExpandedBodyMacro,
    },
    map::{DefMap, DefMapBuilder, PartialDefMap},
    module::{ModuleData, ModuleOrigin},
    package::{CrateData, MacroExpansionLimitGroup, MacroExpansionLimitReport, PackageDefMaps},
    profile::profile_descriptors,
    query::{
        CrateResolutionEnv, DefMapQuery, DefMapSource, GlobImportSource, ImportResolution,
        ImportedScopeBinding, MacroDefinitionEnv, ResolvePathResult, ScopeResolutionEnv,
        ScopeResolver,
    },
    scope::{
        ModuleScope, ModuleScopeBuilder, Namespace, NamespaceSet, PerNs, ScopeBinding,
        ScopeBindingProvenance, ScopeBindingRoute, ScopeEntry, ScopeEntryRef, ScopeResolution,
        ScopeResolutionRef, Visibility,
    },
    source::{
        BodyItemSourceRef, GeneratedItemRef, GeneratedSourceData, GeneratedSourceId, ItemSource,
        ItemSourceKind,
    },
    store::{DefMapDb, DefMapReadTxn, DefMapStats, UnresolvedImportStats},
    visible::{VisibleScopeDef, VisibleScopeDefs, VisibleScopeOrigin},
};

#[cfg(test)]
mod tests;
