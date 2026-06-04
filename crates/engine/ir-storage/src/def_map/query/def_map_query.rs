//! Shared path queries over DefMap storage.
//!
//! DefMaps only know about one scope graph. This query object adds the routing layer that decides
//! which graph owns a `DefMapRef`, then reuses the private path resolver for the actual lookup.

use rg_ir_model::{DefId, DefMapRef, LocalDefRef, LocalImplRef, ModuleRef, TargetRef};
use rg_text::Name;

use super::{
    path_resolution::{NameResolutionFilter, PathResolver, ResolvePathResult},
    resolution_env::{ScopeResolutionEnv, TargetResolutionEnv},
};

use super::super::{
    DefMap, LocalDefData, LocalImplData, MacroDefinitionData, ModuleData, ModuleScopeBuilder,
    Namespace, Path, ScopeEntryRef, ScopeNamespace, VisibleScopeDef, VisibleScopeDefs,
    VisibleScopeOrigin,
};

/// Routes DefMap-origin refs and target-level facts to concrete storage.
///
/// Target-only callers usually delegate to `DefMapReadTxn`; body-aware callers can additionally
/// route the active body origin to its local DefMap without changing the lookup algorithm.
pub trait DefMapSource {
    type Error;

    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, Self::Error>;

    fn extern_root(&self, target: TargetRef, name: &str) -> Result<Option<ModuleRef>, Self::Error>;

    fn extern_roots(&self, target: TargetRef) -> Result<Vec<(String, ModuleRef)>, Self::Error>;

    fn prelude_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, Self::Error>;

    fn root_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, Self::Error>;
}

impl<T: DefMapSource + ?Sized> DefMapSource for &T {
    type Error = T::Error;

    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, Self::Error> {
        (**self).def_map_for_origin(origin)
    }

    fn extern_root(&self, target: TargetRef, name: &str) -> Result<Option<ModuleRef>, Self::Error> {
        (**self).extern_root(target, name)
    }

    fn extern_roots(&self, target: TargetRef) -> Result<Vec<(String, ModuleRef)>, Self::Error> {
        (**self).extern_roots(target)
    }

    fn prelude_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, Self::Error> {
        (**self).prelude_module(target)
    }

    fn root_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, Self::Error> {
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
    ) -> Result<ResolvePathResult, S::Error> {
        PathResolver::new(self).resolve_path(from, path, NameResolutionFilter::AllNamespaces)
    }

    /// Resolves a type-position path using normal Rust module lookup rules.
    pub fn resolve_path_in_type_namespace(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<ResolvePathResult, S::Error> {
        PathResolver::new(self).resolve_path(from, path, NameResolutionFilter::TypesOnly)
    }

    /// Resolves a path through lexical scopes represented as synthetic modules.
    pub fn resolve_lexical_path(
        &self,
        from: ModuleRef,
        path: &Path,
        filter: NameResolutionFilter,
    ) -> Result<ResolvePathResult, S::Error> {
        PathResolver::new(self).resolve_lexical_path(from, path, filter)
    }

    /// Resolves one name inside a concrete lexical module without walking parents.
    pub fn resolve_lexical_name_in_module(
        &self,
        from: ModuleRef,
        module: ModuleRef,
        name: &str,
        filter: NameResolutionFilter,
    ) -> Result<Vec<rg_ir_model::DefId>, S::Error> {
        PathResolver::new(self).resolve_lexical_name_in_module(from, module, name, filter)
    }

    pub fn module_data(&self, module_ref: ModuleRef) -> Result<Option<&ModuleData>, S::Error> {
        Ok(self
            .source
            .def_map_for_origin(module_ref.origin)?
            .and_then(|def_map| def_map.module(module_ref.module)))
    }

    /// Lists modules recorded in one target DefMap without exposing the concrete store.
    pub fn module_refs(&self, target: TargetRef) -> Result<Vec<ModuleRef>, S::Error> {
        Ok(self
            .source
            .def_map_for_origin(DefMapRef::Target(target))?
            .map(|def_map| def_map.module_refs().collect())
            .unwrap_or_default())
    }

    /// Returns targets whose DefMap roots are visible from `root`.
    ///
    /// This is the target-level language visibility closure: the target itself plus targets named
    /// by external roots and preludes reachable from it. It is intentionally separate from package
    /// transaction inclusion, which is only a storage/materialization boundary.
    pub fn visible_targets_from(&self, root: TargetRef) -> Result<Vec<TargetRef>, S::Error> {
        let mut visible_targets = Vec::new();
        let mut pending_targets = vec![root];

        while let Some(target) = pending_targets.pop() {
            if visible_targets.contains(&target) {
                continue;
            }
            visible_targets.push(target);

            for (_, module) in self.source.extern_roots(target)? {
                if let Some(target) = module.origin.as_target_ref() {
                    pending_targets.push(target);
                }
            }

            if let Some(module) = self.source.prelude_module(target)?
                && let Some(target) = module.origin.as_target_ref()
            {
                pending_targets.push(target);
            }
        }

        Ok(visible_targets)
    }

    pub fn local_def_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&LocalDefData>, S::Error> {
        Ok(self
            .source
            .def_map_for_origin(local_def_ref.origin)?
            .and_then(|def_map| def_map.local_def(local_def_ref.local_def)))
    }

    pub fn local_impl_data(
        &self,
        local_impl_ref: LocalImplRef,
    ) -> Result<Option<&LocalImplData>, S::Error> {
        Ok(self
            .source
            .def_map_for_origin(local_impl_ref.origin)?
            .and_then(|def_map| def_map.local_impl(local_impl_ref.local_impl)))
    }

    pub fn macro_definition_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&MacroDefinitionData>, S::Error> {
        Ok(self
            .source
            .def_map_for_origin(local_def_ref.origin)?
            .and_then(|def_map| def_map.macro_definition(local_def_ref.local_def)))
    }

    /// Returns the namespace occupied by one resolved definition.
    pub fn namespace_for_def(&self, def: DefId) -> Result<Option<Namespace>, S::Error> {
        PathResolver::new(self).namespace_for_def(def)
    }

    /// Builds the visibility-filtered scope observed from `importing_module`.
    pub fn visible_scope(
        &self,
        importing_module: ModuleRef,
        source_module: ModuleRef,
    ) -> Result<ModuleScopeBuilder, S::Error> {
        PathResolver::new(self).visible_scope(importing_module, source_module)
    }

    /// Returns definitions from `source_module` that are visible from `importing_module`.
    pub fn visible_scope_defs(
        &self,
        importing_module: ModuleRef,
        source_module: ModuleRef,
    ) -> Result<VisibleScopeDefs, S::Error> {
        let scope = self.visible_scope(importing_module, source_module)?;
        let mut defs = VisibleScopeDefs::new(&scope, VisibleScopeOrigin::ModuleScope, false);
        defs.sort();
        Ok(defs)
    }

    /// Returns names visible from `importing_module` without a qualifier.
    pub fn visible_unqualified_scope_defs(
        &self,
        importing_module: ModuleRef,
    ) -> Result<VisibleScopeDefs, S::Error> {
        let resolver = PathResolver::new(self);

        // First-segment resolution checks the current module scope before extern roots and the
        // standard prelude. Completion follows the same namespace-specific shadowing order.
        let current_scope = resolver.visible_scope(importing_module, importing_module)?;
        let mut defs =
            VisibleScopeDefs::new(&current_scope, VisibleScopeOrigin::ModuleScope, false);

        let target = importing_module.origin.origin_target();
        let mut extern_roots = self.source.extern_roots(target)?;
        extern_roots.sort_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()));
        for (name, module_ref) in extern_roots {
            let label = name;
            defs.push(
                VisibleScopeDef {
                    label,
                    namespace: ScopeNamespace::Types,
                    def: rg_ir_model::DefId::Module(module_ref),
                    origin: VisibleScopeOrigin::ExternRoot,
                },
                false,
            );
        }

        if let Some(prelude) = self.source.prelude_module(target)? {
            let prelude_scope = resolver.visible_scope(importing_module, prelude)?;
            defs.extend(&prelude_scope, VisibleScopeOrigin::Prelude, true);
        }

        defs.sort();
        Ok(defs)
    }
}

impl<S> ScopeResolutionEnv for DefMapQuery<S>
where
    S: DefMapSource,
{
    type Error = S::Error;

    fn module_data(&self, module_ref: ModuleRef) -> Result<Option<&ModuleData>, Self::Error> {
        DefMapQuery::module_data(self, module_ref)
    }

    fn module_scope_entry<'a>(
        &'a self,
        module_ref: ModuleRef,
        name: &str,
    ) -> Result<Option<ScopeEntryRef<'a>>, Self::Error> {
        Ok(self
            .module_data(module_ref)?
            .and_then(|module| module.scope.entry(name))
            .map(|entry| entry.as_ref()))
    }

    fn module_scope_entries<'a>(
        &'a self,
        module_ref: ModuleRef,
    ) -> Result<Vec<(&'a Name, ScopeEntryRef<'a>)>, Self::Error> {
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
    ) -> Result<Option<&LocalDefData>, Self::Error> {
        DefMapQuery::local_def_data(self, local_def_ref)
    }

    fn macro_definition_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&MacroDefinitionData>, Self::Error> {
        DefMapQuery::macro_definition_data(self, local_def_ref)
    }
}

impl<S> TargetResolutionEnv for DefMapQuery<S>
where
    S: DefMapSource,
{
    fn extern_root(&self, target: TargetRef, name: &str) -> Result<Option<ModuleRef>, Self::Error> {
        self.source.extern_root(target, name)
    }

    fn prelude_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, Self::Error> {
        self.source.prelude_module(target)
    }

    fn root_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, Self::Error> {
        self.source.root_module(target)
    }
}
