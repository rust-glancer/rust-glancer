//! Source traits used by path resolution.
//!
//! These traits describe where resolution reads scope data from. The path-walking algorithm lives
//! separately in `path_resolution`, while each storage owner implements this contract next to its
//! own data access methods.

use rg_ir_model::{DefId, LocalDefRef, ModuleRef, TargetRef};
use rg_text::Name;

use super::super::{
    LocalDefData, LocalDefKind, LocalEnumVariantEntry, MacroDefinitionView, ModuleData,
    ScopeEntryRef,
};

/// Minimal scope graph required by path and visibility lookup.
pub trait ScopeResolutionEnv {
    type Error;

    fn module_data(&self, module_ref: ModuleRef) -> Result<Option<&ModuleData>, Self::Error>;

    fn module_scope_entry<'a>(
        &'a self,
        module_ref: ModuleRef,
        name: &str,
    ) -> Result<Option<ScopeEntryRef<'a>>, Self::Error>;

    fn module_scope_entries<'a>(
        &'a self,
        module_ref: ModuleRef,
    ) -> Result<Vec<(&'a Name, ScopeEntryRef<'a>)>, Self::Error>;

    fn local_def_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&LocalDefData>, Self::Error>;

    fn local_enum_variant_entries_for_enum<'a>(
        &'a self,
        enum_def: LocalDefRef,
    ) -> Result<Vec<LocalEnumVariantEntry<'a>>, Self::Error>;

    fn local_def_kind(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<LocalDefKind>, Self::Error> {
        Ok(self
            .local_def_data(local_def_ref)?
            .map(|local_def| local_def.kind))
    }

    fn parent_module(&self, module_ref: ModuleRef) -> Result<Option<ModuleRef>, Self::Error> {
        let Some(module) = self.module_data(module_ref)? else {
            return Ok(None);
        };

        let Some(parent) = module.parent else {
            return Ok(None);
        };

        Ok(Some(ModuleRef {
            origin: module_ref.origin,
            module: parent,
        }))
    }
}

/// Macro-definition facts needed by declarative macro resolution and expansion.
///
/// This stays separate from `ScopeResolutionEnv` because path walking only needs generic scope
/// facts. Macro callers additionally need to classify a resolved binding as an expandable macro
/// and borrow the retained macro body in one step.
pub trait MacroDefinitionEnv: ScopeResolutionEnv {
    /// Return the expandable macro view for `def`, or `None` for non-local/non-macro definitions.
    fn macro_definition_view<'a>(
        &'a self,
        def: DefId,
    ) -> Result<Option<MacroDefinitionView<'a>>, Self::Error>;
}

/// Target-level graph facts needed by normal Rust module path lookup.
pub trait TargetResolutionEnv: ScopeResolutionEnv {
    fn extern_root(&self, target: TargetRef, name: &str) -> Result<Option<ModuleRef>, Self::Error>;

    fn prelude_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, Self::Error>;

    fn root_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, Self::Error>;
}
