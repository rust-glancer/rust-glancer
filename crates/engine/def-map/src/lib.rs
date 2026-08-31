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
    build::{
        DefMapBuildOutput, DefMapBuildProgress, DefMapBuildSession, GeneratedItemStores,
        MacroSourceFileRequest,
    },
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
    module::{ModuleData, ModuleFileSelection, ModuleOrigin},
    package::{
        CrateData, CrateDefMapManifest, MacroExpansionLimitGroup, MacroExpansionLimitReport,
        PackageDefMaps, PackageDefMapsManifest,
    },
    profile::profile_descriptors,
    query::{
        CrateResolutionEnv, DefMapQuery, DefMapSource, GlobImportSource, ImportResolution,
        MacroDefinitionEnv, ResolvePathResult, ScopeResolutionEnv, ScopeResolver,
    },
    scope::{
        ModuleScope, ModuleScopeBuilder, Namespace, NamespaceSet, PerNs, ScopeBinding,
        ScopeBindingProvenance, ScopeBindingRoute, ScopeEntry, ScopeEntryRef, ScopeResolution,
        ScopeResolutionRef, Visibility,
    },
    source::{
        BodyItemSourceRef, GeneratedItemRef, GeneratedItemStore, GeneratedSourceId, ItemSource,
        ItemSourceKind,
    },
    store::{
        DefMapDb, DefMapLoader, DefMapReadTxn, DefMapStats, LoadDefMap, UnresolvedImportStats,
    },
    visible::{VisibleScopeDef, VisibleScopeDefs, VisibleScopeOrigin},
};

#[cfg(test)]
mod tests;
