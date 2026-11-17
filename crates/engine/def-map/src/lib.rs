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
//!
//! Construction starts at `CrateData::build_target`. Body macro expansion has a separate entry
//! point, `BodyMacroExpander`, which reads frozen definitions and returns syntax for body lowering
//! or source queries without changing the saved item scopes.

mod binding;
mod body_macros;
mod build;
mod import;
mod item_scope;
mod local;
mod macro_definition;
mod map;
mod module;
mod package;
mod profile;
mod query;
mod source;
#[doc(hidden)]
pub mod testonly;
mod visible;

pub(crate) use self::build::MacroSourceFileRequest;
pub use self::{
    binding::{NameBinding, NameBindingProvenance, Namespace, NamespaceSet, PerNs, Visibility},
    body_macros::{
        BodyMacroCallOrigin, BodyMacroCallSite, BodyMacroExpander, BodyMacroExpansionOutcome,
        BodyMacroExpansionScope, BodyMacroExprExpansion, BodyMacroExprExpansionOutcome,
        BodyMacroPatExpansionOutcome, BodyMacroStmtExpansionOutcome, BodyMacroTypeExpansionOutcome,
        ExpandedBodyMacro,
    },
    build::{
        CrateBuildOutput, CrateDependencies, ImportCollectionEnv, ImportCollectionOutput,
        ImportCollector, ModuleImportInput, ResolvedModuleImports,
    },
    import::{ImportBinding, ImportData, ImportKind, ImportPath},
    item_scope::{ItemScope, ItemScopeBuilder, NameBindings, NameBindingsView},
    local::{
        LocalDefData, LocalDefKind, LocalEnumVariantData, LocalEnumVariantEntry, LocalImplData,
    },
    macro_definition::{
        MacroDefinitionData, MacroDefinitionKind, MacroDefinitionPayload, MacroDefinitionView,
    },
    map::{DefMap, DefMapBuilder},
    module::{
        ModuleBuilder, ModuleData, ModuleDocumentation, ModuleFileSelection, ModuleOrigin,
        ModuleResolutionView,
    },
    package::{CrateData, MacroExpansionLimitGroup, MacroExpansionLimitReport},
    profile::profile_descriptors,
    query::{
        CrateDataSource, CrateDefMapReader, CrateResolutionEnv, DefMapQuery, DefMapSource,
        DefinitionReader, GlobImportSource, MacroDefinitionEnv, NameResolutionEnv, NameResolver,
        ResolvePathResult,
    },
    source::{
        BodyItemSourceRef, GeneratedItemRef, GeneratedItemStore, GeneratedSourceId, ItemSource,
        ItemSourceKind,
    },
    visible::{VisibleScopeDef, VisibleScopeDefs, VisibleScopeOrigin},
};

#[cfg(test)]
mod tests;
