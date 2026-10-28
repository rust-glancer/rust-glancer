//! Item names, imports, and item-macro expansion for modules and body-local declarations.
//!
//! The name model has three levels:
//! - `NameBinding` selects one definition in one namespace, together with visibility and source
//!   facts used for imports and presentation.
//! - `NameBindings` groups the type/value/macro answers for one spelling. `NameBindingsView`
//!   lets lookup read the same answers during construction and after freezing.
//! - `ItemScope` maps spellings to those bindings and keeps traits imported as `_` separately.
//!
//! Collection builds item scopes from declarations, imports, and macro output. `ImportCollector`
//! adds import answers to mutable tables; `NameResolver` reads names and paths through an
//! environment that can expose either those tables or frozen DefMaps. Lexical variable bindings
//! and type-dependent lookup belong to later Body IR and type analysis.

mod binding;
mod build;
mod import;
mod item_scope;
mod local;
mod macro_expansion;
mod map;
mod module;
mod package;
mod profile;
mod query;
mod source;
mod store;
#[doc(hidden)]
pub mod testonly;
mod visible;

pub use rg_macro_runtime::MacroExpansionPerformancePreference;

pub use self::{
    binding::{NameBinding, NameBindingProvenance, Namespace, NamespaceSet, PerNs, Visibility},
    build::{
        DefMapBuildOutput, GeneratedItemStores, ImportCollectionEnv, ImportCollectionOutput,
        ImportCollector, MacroSourceFileLoader, MacroSourceFileRequest, MacroSourceFileResolution,
        ModuleImportInput, ResolvedModuleImports,
    },
    import::{ImportBinding, ImportData, ImportKind, ImportPath},
    item_scope::{ItemScope, ItemScopeBuilder, NameBindings, NameBindingsView},
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
        NameResolutionEnv, NameResolver, ResolvePathResult,
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
