//! Shared path queries over DefMap storage.
//!
//! DefMaps only know about one scope graph. This query object adds the routing layer that decides
//! which graph owns a `DefMapRef`, then reuses the private path resolver for the actual lookup.

use rg_ir_model::{DefMapRef, LocalDefRef, ModuleRef, TargetRef};
use rg_package_store::PackageStoreError;
use rg_text::Name;

use crate::{
    DefMap, LocalDefData, MacroDefinitionData, ModuleData, Path, ResolvePathResult,
    model::{ModuleScopeBuilder, ScopeEntryRef},
};

use super::{
    path_resolution::{NameResolutionFilter, PathResolver},
    resolution_env::{ScopeResolutionEnv, TargetResolutionEnv},
};

/// Routes DefMap-origin refs and target-level facts to concrete storage.
///
/// Target-only callers usually delegate to `DefMapReadTxn`; body-aware callers can additionally
/// route the active body origin to its local DefMap without changing the lookup algorithm.
pub trait DefMapSource {
    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, PackageStoreError>;

    fn extern_root(
        &self,
        target: TargetRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, PackageStoreError>;

    fn prelude_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError>;

    fn root_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError>;
}

impl<T: DefMapSource + ?Sized> DefMapSource for &T {
    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, PackageStoreError> {
        (**self).def_map_for_origin(origin)
    }

    fn extern_root(
        &self,
        target: TargetRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, PackageStoreError> {
        (**self).extern_root(target, name)
    }

    fn prelude_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        (**self).prelude_module(target)
    }

    fn root_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        (**self).root_module(target)
    }
}

/// Common DefMap lookup API over any source that can route origins to DefMaps.
#[derive(Clone)]
pub struct DefMapQuery<S> {
    source: S,
}

impl<S> DefMapQuery<S>
where
    S: DefMapSource,
{
    pub fn new(source: S) -> Self {
        Self { source }
    }

    /// Resolves a value-position path using normal Rust module lookup rules.
    pub fn resolve_path(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<ResolvePathResult, PackageStoreError> {
        PathResolver::new(self).resolve_path(from, path, NameResolutionFilter::AllNamespaces)
    }

    /// Resolves a type-position path using normal Rust module lookup rules.
    pub fn resolve_path_in_type_namespace(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<ResolvePathResult, PackageStoreError> {
        PathResolver::new(self).resolve_path(from, path, NameResolutionFilter::TypesOnly)
    }

    /// Resolves a path through lexical scopes represented as synthetic modules.
    pub fn resolve_lexical_path(
        &self,
        from: ModuleRef,
        path: &Path,
        filter: NameResolutionFilter,
    ) -> Result<ResolvePathResult, PackageStoreError> {
        PathResolver::new(self).resolve_lexical_path(from, path, filter)
    }

    /// Resolves one name inside a concrete lexical module without walking parents.
    pub fn resolve_lexical_name_in_module(
        &self,
        from: ModuleRef,
        module: ModuleRef,
        name: &str,
        filter: NameResolutionFilter,
    ) -> Result<Vec<rg_ir_model::DefId>, PackageStoreError> {
        PathResolver::new(self).resolve_lexical_name_in_module(from, module, name, filter)
    }

    pub(crate) fn visible_scope(
        &self,
        importing_module: ModuleRef,
        source_module: ModuleRef,
    ) -> Result<ModuleScopeBuilder, PackageStoreError> {
        PathResolver::new(self).visible_scope(importing_module, source_module)
    }
}

impl<S> ScopeResolutionEnv for DefMapQuery<S>
where
    S: DefMapSource,
{
    fn module_data(&self, module_ref: ModuleRef) -> Result<Option<&ModuleData>, PackageStoreError> {
        Ok(self
            .source
            .def_map_for_origin(module_ref.origin)?
            .and_then(|def_map| def_map.module(module_ref.module)))
    }

    fn module_scope_entry<'a>(
        &'a self,
        module_ref: ModuleRef,
        name: &str,
    ) -> Result<Option<ScopeEntryRef<'a>>, PackageStoreError> {
        Ok(self
            .module_data(module_ref)?
            .and_then(|module| module.scope.entry(name))
            .map(|entry| entry.as_ref()))
    }

    fn module_scope_entries<'a>(
        &'a self,
        module_ref: ModuleRef,
    ) -> Result<Vec<(&'a Name, ScopeEntryRef<'a>)>, PackageStoreError> {
        Ok(self
            .module_data(module_ref)?
            .map(|module| {
                module
                    .scope
                    .entries()
                    .map(|(name, entry)| (name, entry.as_ref()))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn local_def_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&LocalDefData>, PackageStoreError> {
        Ok(self
            .source
            .def_map_for_origin(local_def_ref.origin)?
            .and_then(|def_map| def_map.local_def(local_def_ref.local_def)))
    }

    fn macro_definition_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&MacroDefinitionData>, PackageStoreError> {
        Ok(self
            .source
            .def_map_for_origin(local_def_ref.origin)?
            .and_then(|def_map| def_map.macro_definition(local_def_ref.local_def)))
    }
}

impl<S> TargetResolutionEnv for DefMapQuery<S>
where
    S: DefMapSource,
{
    fn extern_root(
        &self,
        target: TargetRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.source.extern_root(target, name)
    }

    fn prelude_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.source.prelude_module(target)
    }

    fn root_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.source.root_module(target)
    }
}
