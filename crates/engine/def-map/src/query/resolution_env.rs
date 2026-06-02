//! Source traits used by path resolution.
//!
//! These traits describe where resolution reads scope data from. The path-walking algorithm lives
//! separately in `path_resolution`, while each storage owner implements this contract next to its
//! own data access methods.

use rg_ir_model::{LocalDefRef, ModuleRef, TargetRef};
use rg_package_store::PackageStoreError;
use rg_text::Name;

use crate::{LocalDefData, LocalDefKind, MacroDefinitionData, ModuleData, ScopeEntryRef};

/// Minimal scope graph required by path and visibility lookup.
pub(crate) trait ScopeResolutionEnv {
    fn module_data(&self, module_ref: ModuleRef) -> Result<Option<&ModuleData>, PackageStoreError>;

    fn module_scope_entry<'a>(
        &'a self,
        module_ref: ModuleRef,
        name: &str,
    ) -> Result<Option<ScopeEntryRef<'a>>, PackageStoreError>;

    fn module_scope_entries<'a>(
        &'a self,
        module_ref: ModuleRef,
    ) -> Result<Vec<(&'a Name, ScopeEntryRef<'a>)>, PackageStoreError>;

    fn local_def_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&LocalDefData>, PackageStoreError>;

    fn macro_definition_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&MacroDefinitionData>, PackageStoreError>;

    fn local_def_kind(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<LocalDefKind>, PackageStoreError> {
        Ok(self
            .local_def_data(local_def_ref)?
            .map(|local_def| local_def.kind))
    }

    fn parent_module(&self, module_ref: ModuleRef) -> Result<Option<ModuleRef>, PackageStoreError> {
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

/// Target-level graph facts needed by normal Rust module path lookup.
pub(crate) trait TargetResolutionEnv: ScopeResolutionEnv {
    fn extern_root(
        &self,
        target: TargetRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, PackageStoreError>;

    fn prelude_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError>;

    fn root_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError>;
}
