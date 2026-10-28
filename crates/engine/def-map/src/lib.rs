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

pub use rg_macro_runtime::MacroExpansionPerformancePreference;

pub use self::{
    build::{
        DefMapBuildOutput, GeneratedItemStores, ImportResolutionEnv, ImportResolutionOutput,
        ImportResolver, MacroSourceFileLoader, MacroSourceFileRequest, MacroSourceFileResolution,
        ModuleImportInput, ResolvedModuleImports,
    },
    import::{ImportBinding, ImportData, ImportKind, ImportPath},
    local::{
        LocalDefData, LocalDefKind, LocalEnumVariantData, LocalEnumVariantEntry, LocalImplData,
        MacroDefinitionData, MacroDefinitionKind, MacroDefinitionPayload, MacroDefinitionView,
    },
    macro_expansion::{
        BodyMacroCallOrigin, BodyMacroCallSite, BodyMacroExpander, BodyMacroExpansionOutcome,
        BodyMacroExpansionScope, BodyMacroExprExpansion, BodyMacroExprExpansionOutcome,
        BodyMacroPatExpansionOutcome, BodyMacroStmtExpansionOutcome, BodyMacroTypeExpansionOutcome,
        ExpandedBodyMacro,
    },
    map::{DefMap, DefMapBuilder, ModuleBuilder},
    module::{
        ModuleData, ModuleDocumentation, ModuleFileSelection, ModuleOrigin, ModuleResolutionView,
    },
    package::{
        CrateData, CrateDefMapManifest, MacroExpansionLimitGroup, MacroExpansionLimitReport,
        PackageDefMaps, PackageDefMapsManifest,
    },
    profile::profile_descriptors,
    query::{
        CrateResolutionEnv, DefMapQuery, DefMapSource, GlobImportSource, MacroDefinitionEnv,
        ResolvePathResult, ScopeResolutionEnv, ScopeResolver,
    },
    scope::{
        ModuleScope, ModuleScopeBuilder, Namespace, NamespaceSet, PerNs, ScopeBinding,
        ScopeBindingProvenance, ScopeEntry, ScopeEntryRef, Visibility,
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
