//! Higher-level queries over routed DefMap storage.
//!
//! DefMaps only know about one scope graph. `DefMapSource` routes origin and crate refs to the
//! concrete storage that owns them; this query object keeps the operations that compose those raw
//! maps into language-shaped answers.

use rg_ir_model::{
    CrateRef, DefId, DefMapRef, ImportRef, LocalDefRef, LocalEnumVariantRef, LocalImplRef,
    ModuleRef,
};
use rg_std::UniqueVec;
use rg_text::Name;
use rustc_hash::FxHashSet;

use super::{
    name_resolution::NameResolver,
    resolution_env::{CrateResolutionEnv, MacroDefinitionEnv, NameResolutionEnv},
};
use crate::{
    DefMap, ImportData, LocalDefData, LocalDefKind, LocalEnumVariantData, LocalEnumVariantEntry,
    LocalImplData, MacroDefinitionView, ModuleData, ModuleOrigin, ModuleResolutionView,
    NameBinding, NameBindingsView, Namespace, NamespaceSet, VisibleScopeDef, VisibleScopeDefs,
    VisibleScopeOrigin,
};

/// Routes DefMap-origin refs and crate-level facts to concrete storage.
///
/// Crate-only callers usually delegate to `DefMapReadTxn`; body-aware callers can additionally
/// route the active body origin to its local DefMap without changing the lookup algorithm.
pub trait DefMapSource {
    type Error;

    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, Self::Error>;

    /// Whether `crate_ref` is a host-side proc-macro implementation crate.
    fn crate_is_proc_macro(&self, crate_ref: CrateRef) -> Result<bool, Self::Error>;

    /// Compilation target width for usize arithmetic, without assuming the host architecture.
    fn target_pointer_width(&self, crate_ref: CrateRef) -> Result<Option<u32>, Self::Error>;

    fn module_data(&self, module_ref: ModuleRef) -> Result<Option<&ModuleData>, Self::Error> {
        Ok(self
            .def_map_for_origin(module_ref.origin)?
            .and_then(|def_map| def_map.module(module_ref.module)))
    }

    fn module_refs(&self, crate_ref: CrateRef) -> Result<Vec<ModuleRef>, Self::Error> {
        Ok(self
            .def_map_for_origin(DefMapRef::Crate(crate_ref))?
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

    fn import_data(&self, import_ref: ImportRef) -> Result<Option<&ImportData>, Self::Error> {
        Ok(self
            .def_map_for_origin(import_ref.origin)?
            .and_then(|def_map| def_map.import(import_ref.import)))
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

    fn extern_root(
        &self,
        crate_ref: CrateRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, Self::Error>;

    fn extern_roots(&self, crate_ref: CrateRef) -> Result<Vec<(String, ModuleRef)>, Self::Error>;

    fn prelude_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, Self::Error>;

    /// Returns the unique dependency crates whose ordinary items are visible from this crate.
    ///
    /// Item lookup needs only crate identities, not the names used to import their roots. A source
    /// backed by compact package directories overrides this method and can therefore walk a chain
    /// such as `app -> library -> core` without loading any of those crates' module scopes. Body-local
    /// and test sources can use this default, which derives the same identities from extern roots and
    /// the standard prelude.
    fn item_lookup_dependencies(
        &self,
        crate_ref: CrateRef,
    ) -> Result<UniqueVec<CrateRef>, Self::Error> {
        let mut dependencies = self
            .extern_roots(crate_ref)?
            .into_iter()
            .filter_map(|(_, module)| module.origin.as_crate_ref())
            .collect::<UniqueVec<_>>();
        if let Some(module) = self.prelude_module(crate_ref)?
            && let Some(crate_ref) = module.origin.as_crate_ref()
        {
            dependencies.push(crate_ref);
        }
        Ok(dependencies)
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, Self::Error>;
}

impl<T: DefMapSource + ?Sized> DefMapSource for &T {
    type Error = T::Error;

    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, Self::Error> {
        (**self).def_map_for_origin(origin)
    }

    fn crate_is_proc_macro(&self, crate_ref: CrateRef) -> Result<bool, Self::Error> {
        (**self).crate_is_proc_macro(crate_ref)
    }

    fn target_pointer_width(&self, crate_ref: CrateRef) -> Result<Option<u32>, Self::Error> {
        (**self).target_pointer_width(crate_ref)
    }

    fn extern_root(
        &self,
        crate_ref: CrateRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, Self::Error> {
        (**self).extern_root(crate_ref, name)
    }

    fn extern_roots(&self, crate_ref: CrateRef) -> Result<Vec<(String, ModuleRef)>, Self::Error> {
        (**self).extern_roots(crate_ref)
    }

    fn prelude_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        (**self).prelude_module(crate_ref)
    }

    fn item_lookup_dependencies(
        &self,
        crate_ref: CrateRef,
    ) -> Result<UniqueVec<CrateRef>, Self::Error> {
        (**self).item_lookup_dependencies(crate_ref)
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        (**self).root_module(crate_ref)
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

    pub fn target_pointer_width(&self, crate_ref: CrateRef) -> Result<Option<u32>, S::Error> {
        self.source.target_pointer_width(crate_ref)
    }

    /// Construct a name resolver over this routed DefMap source.
    pub fn name_resolver(&self) -> NameResolver<'_, Self> {
        NameResolver::new(self)
    }

    /// Returns crates whose ordinary semantic items can participate in lookup from `root`.
    ///
    /// A proc-macro dependency exposes macro identities, but its implementation is a host-side
    /// program rather than a Rust library linked into the consuming crate. Stop traversal at that
    /// edge so neither the implementation store nor its dependencies contribute methods, impls,
    /// or language items to the consumer. The root is always included: while analysing a
    /// proc-macro crate, its own functions and normal dependencies are ordinary local semantics.
    ///
    /// For example, `app -> runtime -> derive_macro -> parser` contributes `app` and `runtime` to
    /// the app's item lookup. Analysing `derive_macro` itself contributes `derive_macro` and
    /// `parser`. Each crate appears at most once in traversal order.
    pub fn item_lookup_crates_from(&self, root: CrateRef) -> Result<UniqueVec<CrateRef>, S::Error> {
        let mut visible_crates = UniqueVec::new();
        let mut visited_crates = UniqueVec::new();
        let mut pending_crates = vec![root];

        while let Some(crate_ref) = pending_crates.pop() {
            if !visited_crates.push(crate_ref) {
                continue;
            }

            if crate_ref != root && self.source.crate_is_proc_macro(crate_ref)? {
                continue;
            }
            visible_crates.push(crate_ref);

            pending_crates.extend(self.source.item_lookup_dependencies(crate_ref)?);
        }

        Ok(visible_crates)
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
        let mut defs = VisibleScopeDefs::empty();
        self.name_resolver().visit_visible_bindings(
            importing_module,
            source_module,
            NamespaceSet::ALL,
            |name, namespace, binding| {
                defs.push_binding(
                    name,
                    namespace,
                    binding,
                    VisibleScopeOrigin::ItemScope,
                    false,
                );
                Ok(())
            },
        )?;
        defs.sort();
        Ok(defs)
    }

    /// Returns traits contributed by a lexical scope and its enclosing synthetic scopes.
    ///
    /// Trait candidates do not use ordinary path shadowing between nested scopes:
    ///
    /// ```text
    /// use api::Render;
    /// {
    ///     struct Render;
    ///     value.render(); // the outer trait remains eligible
    /// }
    /// ```
    ///
    /// Binding selection still happens *within* each scope. For example, a local type can suppress
    /// a same-named glob import before that imported trait enters this candidate set. Imports using
    /// `as _` join the set through the separate unnamed lane because they occupy no path name.
    pub fn traits_in_lexical_scope(
        &self,
        importing_module: ModuleRef,
    ) -> Result<UniqueVec<LocalDefRef>, S::Error> {
        let resolver = self.name_resolver();
        let mut traits = UniqueVec::new();
        let mut current = Some(importing_module);
        while let Some(module_ref) = current {
            resolver.visit_visible_bindings(
                importing_module,
                module_ref,
                NamespaceSet::TYPES,
                |_, _, binding| self.push_named_trait(&mut traits, binding),
            )?;

            let Some(module) = self.source.module_data(module_ref)? else {
                break;
            };
            self.push_unnamed_traits(&mut traits, &resolver, importing_module, module)?;

            if !matches!(module.origin, ModuleOrigin::Synthetic { .. }) {
                break;
            }
            current = module.parent.map(|module| ModuleRef {
                origin: module_ref.origin,
                module,
            });
        }

        Ok(traits)
    }

    /// Returns traits available to unqualified lookup from one ordinary module.
    ///
    /// The standard prelude behaves like a low-priority glob: a selected type binding in the
    /// module suppresses the same prelude spelling. Underscore imports from the module remain
    /// method candidates, while underscore imports inside the prelude module itself do not
    /// propagate through that implicit glob.
    pub fn traits_in_unqualified_scope(
        &self,
        importing_module: ModuleRef,
    ) -> Result<UniqueVec<LocalDefRef>, S::Error> {
        let resolver = self.name_resolver();
        let mut occupied_type_names = FxHashSet::default();
        let mut traits = UniqueVec::new();
        resolver.visit_visible_bindings(
            importing_module,
            importing_module,
            NamespaceSet::TYPES,
            |name, _, binding| {
                occupied_type_names.insert(name.clone());
                self.push_named_trait(&mut traits, binding)
            },
        )?;
        if let Some(module) = self.source.module_data(importing_module)? {
            self.push_unnamed_traits(&mut traits, &resolver, importing_module, module)?;
        }

        let crate_ref = importing_module.origin.origin_crate();
        if let Some(prelude) = self.source.prelude_module(crate_ref)? {
            resolver.visit_visible_bindings(
                importing_module,
                prelude,
                NamespaceSet::TYPES,
                |name, _, binding| {
                    if !occupied_type_names.contains(name) {
                        self.push_named_trait(&mut traits, binding)?;
                    }
                    Ok(())
                },
            )?;
        }

        Ok(traits)
    }

    /// Add a selected type binding when it denotes a trait.
    fn push_named_trait(
        &self,
        traits: &mut UniqueVec<LocalDefRef>,
        binding: &NameBinding,
    ) -> Result<(), S::Error> {
        if let DefId::Local(local_def) = binding.def
            && self.source.local_def_data(local_def)?.map(|data| data.kind)
                == Some(LocalDefKind::Trait)
        {
            traits.push(local_def);
        }
        Ok(())
    }

    /// Add visible `as _` imports stored outside the ordinary namespace slots.
    fn push_unnamed_traits(
        &self,
        traits: &mut UniqueVec<LocalDefRef>,
        resolver: &NameResolver<'_, Self>,
        importing_module: ModuleRef,
        module: &ModuleData,
    ) -> Result<(), S::Error> {
        for binding in module.scope.unnamed_trait_bindings() {
            if resolver.binding_is_visible(importing_module, binding)?
                && let DefId::Local(local_def) = binding.def
            {
                traits.push(local_def);
            }
        }
        Ok(())
    }

    /// Returns names visible from `importing_module` without a qualifier.
    pub fn visible_unqualified_scope_defs(
        &self,
        importing_module: ModuleRef,
    ) -> Result<VisibleScopeDefs, S::Error> {
        let resolver = self.name_resolver();

        // First-segment resolution checks the current module scope before extern roots and the
        // standard prelude. Completion follows the same namespace-specific shadowing order.
        let mut defs = VisibleScopeDefs::empty();
        resolver.visit_visible_bindings(
            importing_module,
            importing_module,
            NamespaceSet::ALL,
            |name, namespace, binding| {
                defs.push_binding(
                    name,
                    namespace,
                    binding,
                    VisibleScopeOrigin::ItemScope,
                    false,
                );
                Ok(())
            },
        )?;

        let crate_ref = importing_module.origin.origin_crate();
        for visible in self.visible_absolute_root_defs(importing_module)? {
            defs.push(visible, true);
        }

        if let Some(prelude) = self.source.prelude_module(crate_ref)? {
            resolver.visit_visible_bindings(
                importing_module,
                prelude,
                NamespaceSet::ALL,
                |name, namespace, binding| {
                    defs.push_binding(name, namespace, binding, VisibleScopeOrigin::Prelude, true);
                    Ok(())
                },
            )?;
        }

        defs.sort();
        Ok(defs)
    }

    /// Returns names available immediately after a leading absolute `::`.
    ///
    /// Unlike ordinary unqualified lookup, this namespace contains only extern-prelude roots. It
    /// is exposed separately so completion and resolution agree that `::std` does not consult the
    /// current module or standard prelude items.
    pub fn visible_absolute_root_defs(
        &self,
        importing_module: ModuleRef,
    ) -> Result<VisibleScopeDefs, S::Error> {
        let crate_ref = importing_module.origin.origin_crate();
        let mut extern_roots = self.source.extern_roots(crate_ref)?;
        extern_roots.sort_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()));

        let mut defs = VisibleScopeDefs::empty();
        for (label, module_ref) in extern_roots {
            defs.push(
                VisibleScopeDef {
                    label,
                    namespace: Namespace::Types,
                    def: rg_ir_model::DefId::Module(module_ref),
                    origin: VisibleScopeOrigin::ExternRoot,
                    attribute_import: None,
                    has_reexport: false,
                },
                false,
            );
        }
        defs.sort();
        Ok(defs)
    }
}

impl<S> NameResolutionEnv for DefMapQuery<S>
where
    S: DefMapSource,
{
    type Error = S::Error;

    fn module_metadata(
        &self,
        module_ref: ModuleRef,
    ) -> Result<Option<ModuleResolutionView<'_>>, Self::Error> {
        Ok(self
            .source
            .module_data(module_ref)?
            .map(ModuleData::resolution_view))
    }

    fn module_scope_entry<'a>(
        &'a self,
        module_ref: ModuleRef,
        name: &str,
    ) -> Result<Option<NameBindingsView<'a>>, Self::Error> {
        Ok(self
            .source
            .module_data(module_ref)?
            .and_then(|module| module.scope.entry(name))
            .map(|entry| entry.as_view()))
    }

    fn module_scope_entries<'a>(
        &'a self,
        module_ref: ModuleRef,
    ) -> Result<Vec<(&'a Name, NameBindingsView<'a>)>, Self::Error> {
        Ok(self
            .source
            .module_data(module_ref)?
            .map(|module| {
                module
                    .scope
                    .entries()
                    .map(|(name, entry)| (name, entry.as_view()))
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

    fn local_enum_variant_data(
        &self,
        variant_ref: LocalEnumVariantRef,
    ) -> Result<Option<&LocalEnumVariantData>, Self::Error> {
        self.source.local_enum_variant_data(variant_ref)
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

impl<S> CrateResolutionEnv for DefMapQuery<S>
where
    S: DefMapSource,
{
    fn extern_root(
        &self,
        crate_ref: CrateRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, Self::Error> {
        self.source.extern_root(crate_ref, name)
    }

    fn prelude_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        self.source.prelude_module(crate_ref)
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        self.source.root_module(crate_ref)
    }
}
