//! Higher-level queries over routed DefMap storage.
//!
//! DefMaps only know about one scope graph. `DefMapSource` routes origin and target refs to the
//! concrete storage that owns them; this query object keeps the operations that compose those raw
//! maps into language-shaped answers.

use rg_ir_model::{
    DefId, DefMapRef, LocalDefRef, LocalEnumVariantRef, LocalImplRef, ModuleRef, TargetRef,
};
use rg_text::Name;

use super::{
    path_resolution::ScopeResolver,
    resolution_env::{MacroDefinitionEnv, ScopeResolutionEnv, TargetResolutionEnv},
};

use super::super::{
    DefMap, LocalDefData, LocalEnumVariantData, LocalEnumVariantEntry, LocalImplData,
    MacroDefinitionView, ModuleData, ScopeEntryRef, ScopeNamespace, VisibleScopeDef,
    VisibleScopeDefs, VisibleScopeOrigin,
};

/// Routes DefMap-origin refs and target-level facts to concrete storage.
///
/// Target-only callers usually delegate to `DefMapReadTxn`; body-aware callers can additionally
/// route the active body origin to its local DefMap without changing the lookup algorithm.
pub trait DefMapSource {
    type Error;

    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, Self::Error>;

    fn module_data(&self, module_ref: ModuleRef) -> Result<Option<&ModuleData>, Self::Error> {
        Ok(self
            .def_map_for_origin(module_ref.origin)?
            .and_then(|def_map| def_map.module(module_ref.module)))
    }

    fn module_refs(&self, target: TargetRef) -> Result<Vec<ModuleRef>, Self::Error> {
        Ok(self
            .def_map_for_origin(DefMapRef::Target(target))?
            .map(|def_map| def_map.module_refs().collect())
            .unwrap_or_default())
    }

    fn local_def_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&LocalDefData>, Self::Error> {
        Ok(self
            .def_map_for_origin(local_def_ref.origin)?
            .and_then(|def_map| def_map.local_def(local_def_ref.local_def)))
    }

    fn local_impl_data(
        &self,
        local_impl_ref: LocalImplRef,
    ) -> Result<Option<&LocalImplData>, Self::Error> {
        Ok(self
            .def_map_for_origin(local_impl_ref.origin)?
            .and_then(|def_map| def_map.local_impl(local_impl_ref.local_impl)))
    }

    fn local_enum_variant_data(
        &self,
        variant_ref: LocalEnumVariantRef,
    ) -> Result<Option<&LocalEnumVariantData>, Self::Error> {
        Ok(self
            .def_map_for_origin(variant_ref.origin)?
            .and_then(|def_map| def_map.local_enum_variant(variant_ref.local_enum_variant)))
    }

    fn local_enum_variant_entries_for_enum<'a>(
        &'a self,
        enum_def: LocalDefRef,
    ) -> Result<Vec<LocalEnumVariantEntry<'a>>, Self::Error> {
        Ok(self
            .def_map_for_origin(enum_def.origin)?
            .map(|def_map| {
                def_map
                    .local_enum_variant_entries_for_enum(enum_def.local_def)
                    .collect()
            })
            .unwrap_or_default())
    }

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

/// Composed DefMap queries over any source that can route origins to DefMaps.
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

    /// Construct a scope resolver over this routed DefMap source.
    pub fn scope_resolver(&self) -> ScopeResolver<'_, Self> {
        ScopeResolver::new(self)
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

    /// Classify a resolved definition as a declarative macro and borrow its expansion payload.
    pub fn macro_definition_view(
        &self,
        def: DefId,
    ) -> Result<Option<MacroDefinitionView<'_>>, S::Error> {
        if let DefId::Local(def_ref) = def
            && let Some(def_map) = self.source.def_map_for_origin(def_ref.origin)?
            && let Some(local_def) = def_map.local_def(def_ref.local_def)
            && let Some(data) = def_map.macro_definition(def_ref.local_def)
        {
            Ok(MacroDefinitionView::new(def_ref, local_def, data))
        } else {
            Ok(None)
        }
    }

    /// Returns definitions from `source_module` that are visible from `importing_module`.
    pub fn visible_scope_defs(
        &self,
        importing_module: ModuleRef,
        source_module: ModuleRef,
    ) -> Result<VisibleScopeDefs, S::Error> {
        let scope = self
            .scope_resolver()
            .visible_scope(importing_module, source_module)?;
        let mut defs = VisibleScopeDefs::new(&scope, VisibleScopeOrigin::ModuleScope, false);
        defs.sort();
        Ok(defs)
    }

    /// Returns names visible from `importing_module` without a qualifier.
    pub fn visible_unqualified_scope_defs(
        &self,
        importing_module: ModuleRef,
    ) -> Result<VisibleScopeDefs, S::Error> {
        let resolver = self.scope_resolver();

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
        self.source.module_data(module_ref)
    }

    fn module_scope_entry<'a>(
        &'a self,
        module_ref: ModuleRef,
        name: &str,
    ) -> Result<Option<ScopeEntryRef<'a>>, Self::Error> {
        Ok(<Self as ScopeResolutionEnv>::module_data(self, module_ref)?
            .and_then(|module| module.scope.entry(name))
            .map(|entry| entry.as_ref()))
    }

    fn module_scope_entries<'a>(
        &'a self,
        module_ref: ModuleRef,
    ) -> Result<Vec<(&'a Name, ScopeEntryRef<'a>)>, Self::Error> {
        Ok(<Self as ScopeResolutionEnv>::module_data(self, module_ref)?
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
        self.source.local_def_data(local_def_ref)
    }

    fn local_enum_variant_entries_for_enum<'a>(
        &'a self,
        enum_def: LocalDefRef,
    ) -> Result<Vec<LocalEnumVariantEntry<'a>>, Self::Error> {
        self.source.local_enum_variant_entries_for_enum(enum_def)
    }
}

impl<S> MacroDefinitionEnv for DefMapQuery<S>
where
    S: DefMapSource,
{
    fn macro_definition_view<'a>(
        &'a self,
        def: DefId,
    ) -> Result<Option<MacroDefinitionView<'a>>, Self::Error> {
        DefMapQuery::macro_definition_view(self, def)
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
