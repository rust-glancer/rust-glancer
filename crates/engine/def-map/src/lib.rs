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
mod store;
#[doc(hidden)]
pub mod testonly;
mod visible;

pub use rg_workspace::PackageSlot;

pub use rg_macro_runtime::MacroExpansionPerformancePreference;

pub use self::{
    import::{ImportBinding, ImportData, ImportKind, ImportPath},
    local::{
        LocalDefData, LocalDefKind, LocalEnumVariantData, LocalEnumVariantEntry, LocalImplData,
        MacroDefinitionData, MacroDefinitionPayload, MacroDefinitionView,
    },
    macro_expansion::{
        BodyMacroCallOrigin, BodyMacroCallSite, BodyMacroExpander, BodyMacroExpansionOutcome,
        BodyMacroExprExpansion, BodyMacroExprExpansionOutcome, BodyMacroPatExpansionOutcome,
        BodyMacroStmtExpansionOutcome, BodyMacroTypeExpansionOutcome, ExpandedBodyMacro,
    },
    map::{DefMap, DefMapBuilder, PartialDefMap},
    module::{ModuleData, ModuleOrigin},
    package::{PackageDefMaps, TargetData},
    profile::profile_descriptors,
    query::{
        DefMapCursorCandidate, DefMapPathCompletionSite, DefMapQuery, DefMapSource,
        DefMapUnqualifiedCompletionSite, GlobImportSource, ImportResolution, ImportedScopeBinding,
        MacroDefinitionEnv, ResolvePathResult, ScopeResolutionEnv, ScopeResolver,
        TargetResolutionEnv,
    },
    scope::{
        ModuleScope, ModuleScopeBuilder, Namespace, NamespaceSet, PerNs, ScopeBinding,
        ScopeBindingProvenance, ScopeBindingRoute, ScopeEntry, ScopeEntryRef, ScopeResolution,
        ScopeResolutionRef, Visibility,
    },
    store::{DefMapDb, DefMapReadTxn, DefMapStats},
    visible::{VisibleScopeDef, VisibleScopeDefs, VisibleScopeOrigin},
};

#[cfg(test)]
mod tests;
