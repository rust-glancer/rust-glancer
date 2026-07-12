//! Scope-graph name lookup for DefMap-backed module scopes.
//!
//! DefMap finalization, body-local DefMap finalization, and ordinary queries all need the same
//! lookup rules while reading from different storage. This module owns those shared rules:
//! lexical/body lookup, module path lookup, import expansion, visibility filtering, and the small
//! macro-namespace queries needed before macro-specific precedence is applied.
//!
//! The resolver does not own scopes. It reads through `ScopeResolutionEnv` and
//! `TargetResolutionEnv`, so finalization can pass current fixed-point snapshots while frozen
//! queries read persisted DefMaps.

use rg_ir_model::{DefId, ImportRef, LocalDefRef, ModuleRef, Path, PathSegment};
use rg_std::UniqueVec;
use rg_text::Name;

use super::super::{
    ImportData, ImportKind, LocalDefKind, ModuleOrigin, ModuleScopeBuilder, Namespace,
    NamespaceSet, ScopeBinding, ScopeBindingProvenance, Visibility,
};

use super::resolution_env::{ScopeResolutionEnv, TargetResolutionEnv};

/// Macro candidates kept in the buckets required by Rust lookup precedence.
///
/// Callers try `module_scope` before `standard_prelude`; this type only preserves the split while
/// sharing the visibility walk.
pub struct UnqualifiedMacroBindings {
    pub module_scope: Vec<ScopeBinding>,
    pub standard_prelude: Vec<ScopeBinding>,
}

/// Result of walking a path through the current scope graph.
///
/// `unresolved_at` points at the first segment that could not be resolved. Keeping that status
/// explicit lets callers report partial resolution without inferring failure from `resolved`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvePathResult {
    pub resolved: Vec<DefId>,
    pub unresolved_at: Option<usize>,
}

/// Item that can appear on the left side of a glob import.
///
/// Modules export visible scope bindings. Enums export visible variant constructors into the value
/// namespace, as in `use Option::*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobImportSource {
    Module(ModuleRef),
    Enum(LocalDefRef),
}

/// One name and namespace slot introduced by a resolved import directive.
///
/// A single `use Unit as LocalUnit` can produce two of these facts: one for the struct type and one
/// for its unit constructor in the value namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedScopeBinding {
    pub name: Name,
    pub namespace: Namespace,
    pub binding: ScopeBinding,
}

/// Result of resolving one import against a fixed-point scope snapshot.
///
/// A source can resolve without introducing a named binding, as with `use path as _`. Keeping that
/// status separate lets import application and unresolved-import reporting share one authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportResolution {
    /// Binding facts for the caller to insert into its mutable scope storage.
    pub introduced: Vec<ImportedScopeBinding>,
    source_resolved: bool,
}

impl ImportResolution {
    /// Whether the source path resolved, even if the import intentionally introduced no name.
    pub fn is_resolved(&self) -> bool {
        self.source_resolved
    }
}

/// Applies name lookup rules over one abstract scope source.
///
/// `ScopeResolver` is storage-agnostic on purpose. During finalization the environment can expose
/// partial builders plus the current fixed-point scope snapshot; after finalization the same lookup
/// code reads frozen DefMaps through `DefMapQuery`.
pub struct ScopeResolver<'env, E: ?Sized> {
    env: &'env E,
}

impl<'env, E: ?Sized> ScopeResolver<'env, E> {
    pub fn new(env: &'env E) -> Self {
        Self { env }
    }

    fn unresolved_at(segment: usize) -> ResolvePathResult {
        ResolvePathResult {
            resolved: Vec::new(),
            unresolved_at: Some(segment),
        }
    }
}

impl<E: ScopeResolutionEnv + ?Sized> ScopeResolver<'_, E> {
    /// Return every namespace a resolved definition occupies when inserted through an import.
    pub fn namespaces_for_def(&self, def: DefId) -> Result<NamespaceSet, E::Error> {
        match def {
            DefId::Module(_) => Ok(NamespaceSet::TYPES),
            DefId::Local(local_def_ref) => Ok(self
                .env
                .local_def_data(local_def_ref)?
                .map(|data| data.namespaces)
                .unwrap_or(NamespaceSet::EMPTY)),
            DefId::EnumVariant(variant_ref) => Ok(self
                .env
                .local_enum_variant_data(variant_ref)?
                .map(|data| data.namespaces)
                .unwrap_or(NamespaceSet::EMPTY)),
        }
    }

    /// Walk a path through lexical scopes without module-keyword or target fallback rules.
    ///
    /// Body-local paths use this form because synthetic modules represent nested lexical scopes,
    /// not full Rust modules with extern roots and preludes.
    pub fn resolve_lexical_path(
        &self,
        importing_module: ModuleRef,
        path: &Path,
        terminal_filter: NamespaceSet,
    ) -> Result<ResolvePathResult, E::Error> {
        if path.absolute {
            return Ok(Self::unresolved_at(0));
        }

        let Some((first_segment, remaining_segments)) = path.segments.split_first() else {
            return Ok(Self::unresolved_at(0));
        };
        let PathSegment::Name(name) = first_segment else {
            return Ok(Self::unresolved_at(0));
        };

        let mut current_defs = self.first_name_in_lexical_scope(
            importing_module,
            name.as_str(),
            NamespaceSet::for_segment(!remaining_segments.is_empty(), terminal_filter),
        )?;
        if current_defs.is_empty() {
            return Ok(Self::unresolved_at(0));
        }

        for (segment_idx, segment) in remaining_segments.iter().enumerate() {
            let PathSegment::Name(name) = segment else {
                return Ok(Self::unresolved_at(segment_idx + 1));
            };
            current_defs = self.lexical_next_name_segment(
                importing_module,
                current_defs,
                name.as_str(),
                NamespaceSet::for_segment(
                    segment_idx + 1 < remaining_segments.len(),
                    terminal_filter,
                ),
            )?;

            if current_defs.is_empty() {
                return Ok(Self::unresolved_at(segment_idx + 1));
            }
        }

        Ok(ResolvePathResult {
            resolved: current_defs,
            unresolved_at: None,
        })
    }

    /// Resolve one name inside a single lexical module without walking parent scopes.
    pub fn resolve_lexical_name_in_module(
        &self,
        importing_module: ModuleRef,
        module_ref: ModuleRef,
        name: &str,
        filter: NamespaceSet,
    ) -> Result<Vec<DefId>, E::Error> {
        let Some(scope_entry) = self.env.module_scope_entry(module_ref, name)? else {
            return Ok(Vec::new());
        };

        let mut defs = UniqueVec::new();
        for namespace in filter.iter() {
            for binding in scope_entry.bindings(namespace) {
                if self.binding_is_visible(importing_module, binding)? {
                    defs.push(binding.def);
                }
            }
        }

        Ok(defs.into_vec())
    }

    fn lexical_next_name_segment(
        &self,
        importing_module: ModuleRef,
        current_defs: Vec<DefId>,
        name: &str,
        filter: NamespaceSet,
    ) -> Result<Vec<DefId>, E::Error> {
        let mut next_defs = UniqueVec::new();

        for current_def in current_defs {
            match current_def {
                DefId::Module(module_ref) => {
                    for resolved_def in self.resolve_lexical_name_in_module(
                        importing_module,
                        module_ref,
                        name,
                        filter,
                    )? {
                        next_defs.push(resolved_def);
                    }
                }
                DefId::Local(local_def_ref) => {
                    if let Some(variant) =
                        self.enum_variant_for_name(local_def_ref, name, filter)?
                    {
                        next_defs.push(DefId::EnumVariant(variant));
                    }
                }
                DefId::EnumVariant(_) => {}
            }
        }

        Ok(next_defs.into_vec())
    }

    fn enum_variant_for_name(
        &self,
        enum_def: LocalDefRef,
        name: &str,
        filter: NamespaceSet,
    ) -> Result<Option<rg_ir_model::LocalEnumVariantRef>, E::Error> {
        if !self
            .env
            .local_def_kind(enum_def)?
            .is_some_and(|kind| kind == LocalDefKind::Enum)
        {
            return Ok(None);
        }

        Ok(self
            .env
            .local_enum_variant_entries_for_enum(enum_def)?
            .into_iter()
            .find_map(|entry| {
                (entry.data.name == name && entry.data.namespaces.intersects(filter))
                    .then_some(entry.variant_ref)
            }))
    }

    fn first_name_in_lexical_scope(
        &self,
        importing_module: ModuleRef,
        name: &str,
        filter: NamespaceSet,
    ) -> Result<Vec<DefId>, E::Error> {
        let mut current = Some(importing_module);
        while let Some(module_ref) = current {
            let defs =
                self.resolve_lexical_name_in_module(importing_module, module_ref, name, filter)?;
            if !defs.is_empty() {
                return Ok(defs);
            }

            let Some(module) = self.env.module_data(module_ref)? else {
                return Ok(Vec::new());
            };
            if !matches!(module.origin, ModuleOrigin::Synthetic { .. }) {
                break;
            }
            current = self.env.parent_module(module_ref)?;
        }

        Ok(Vec::new())
    }

    /// Checks whether any retained route to a selected binding is visible from a module.
    pub fn binding_is_visible(
        &self,
        importing_module: ModuleRef,
        binding: &ScopeBinding,
    ) -> Result<bool, E::Error> {
        for route in binding.routes() {
            if self.visibility_is_visible(importing_module, route.visibility)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Create the route introduced by an import without widening the source definition.
    ///
    /// Each visible source route is intersected with the use item's visibility. Legacy
    /// `macro_rules!` is the one special case: its direct route may be re-exported up to the
    /// defining crate, while `#[macro_export]` may make it public.
    pub fn imported_binding(
        &self,
        importing_module: ModuleRef,
        source: &ScopeBinding,
        import_visibility: Visibility,
        provenance: ScopeBindingProvenance,
    ) -> Result<Option<ScopeBinding>, E::Error> {
        let mut imported: Option<ScopeBinding> = None;

        for route in source.routes() {
            if !self.visibility_is_visible(importing_module, route.visibility)? {
                continue;
            }
            // A direct `macro_rules!` binding has textual visibility, but a named import may expose
            // it anywhere inside the defining crate. It still cannot become public outside that
            // crate unless `#[macro_export]` contributed a separate public root route.
            let source_visibility = if route.provenance == ScopeBindingProvenance::DirectMacroRules
            {
                self.local_def_crate_visibility(source.def)?
                    .unwrap_or(route.visibility)
            } else if route.provenance == ScopeBindingProvenance::DirectMacroExport {
                Visibility::Public
            } else {
                route.visibility
            };
            let Some(visibility) =
                self.intersect_visibility(source_visibility, import_visibility)?
            else {
                continue;
            };

            let candidate = ScopeBinding::new(source.def, visibility, provenance);
            if let Some(existing) = &mut imported {
                existing.merge_routes(candidate);
            } else {
                imported = Some(candidate);
            }
        }

        Ok(imported)
    }

    fn local_def_crate_visibility(&self, def: DefId) -> Result<Option<Visibility>, E::Error> {
        let DefId::Local(local_def) = def else {
            return Ok(None);
        };
        let Some(data) = self.env.local_def_data(local_def)? else {
            return Ok(None);
        };

        let mut root = ModuleRef {
            origin: local_def.origin,
            module: data.module,
        };
        while let Some(parent) = self.env.parent_module(root)? {
            root = parent;
        }
        Ok(Some(Visibility::Module(root)))
    }

    fn visibility_is_visible(
        &self,
        importing_module: ModuleRef,
        visibility: Visibility,
    ) -> Result<bool, E::Error> {
        match visibility {
            Visibility::Public => Ok(true),
            Visibility::Module(visible_from) => {
                self.module_is_descendant_of(importing_module, visible_from)
            }
            Visibility::Invisible => Ok(false),
        }
    }

    /// Intersects two descendant-subtree visibility regions.
    fn intersect_visibility(
        &self,
        left: Visibility,
        right: Visibility,
    ) -> Result<Option<Visibility>, E::Error> {
        match (left, right) {
            (Visibility::Invisible, _) | (_, Visibility::Invisible) => Ok(None),
            (Visibility::Public, visibility) | (visibility, Visibility::Public) => {
                Ok(Some(visibility))
            }
            (Visibility::Module(left), Visibility::Module(right)) => {
                if self.module_is_descendant_of(left, right)? {
                    Ok(Some(Visibility::Module(left)))
                } else if self.module_is_descendant_of(right, left)? {
                    Ok(Some(Visibility::Module(right)))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Walk parent links inside one module origin to test Rust's ancestor-based visibility.
    fn module_is_descendant_of(
        &self,
        module: ModuleRef,
        ancestor: ModuleRef,
    ) -> Result<bool, E::Error> {
        if module.origin != ancestor.origin {
            return Ok(false);
        }

        let mut current = Some(module.module);
        while let Some(module_id) = current {
            if module_id == ancestor.module {
                return Ok(true);
            }

            current = self
                .env
                .module_data(ModuleRef {
                    origin: module.origin,
                    module: module_id,
                })?
                .and_then(|module| module.parent);
        }

        Ok(false)
    }
}

impl<E: TargetResolutionEnv + ?Sized> ScopeResolver<'_, E> {
    /// Resolve one import and return every binding it introduces.
    ///
    /// Both target and body DefMap builders apply these facts to their own mutable scope storage.
    /// This method does not mutate either scope: the same operation can be used again after the
    /// fixed point to classify unresolved imports. Lookup, provenance, and visibility intersection
    /// therefore have one authority.
    pub fn resolve_import(
        &self,
        importing_module: ModuleRef,
        import_ref: ImportRef,
        import: &ImportData,
    ) -> Result<ImportResolution, E::Error> {
        match import.kind {
            ImportKind::Glob => {
                let sources = self.import_glob_sources(importing_module, import.path.semantic())?;
                let source_resolved = !sources.is_empty();
                let mut introduced = Vec::new();

                for source in sources {
                    let source_scope = self.visible_glob_source_scope(importing_module, source)?;
                    for (name, entry) in source_scope.entries() {
                        for namespace in Namespace::ALL {
                            for source_binding in entry.bindings(namespace) {
                                let Some(binding) = self.imported_binding(
                                    importing_module,
                                    source_binding,
                                    import.visibility,
                                    ScopeBindingProvenance::GlobImport(import_ref),
                                )?
                                else {
                                    continue;
                                };
                                introduced.push(ImportedScopeBinding {
                                    name: name.clone(),
                                    namespace,
                                    binding,
                                });
                            }
                        }
                    }
                }

                Ok(ImportResolution {
                    introduced,
                    source_resolved,
                })
            }
            ImportKind::Named | ImportKind::SelfImport => {
                let source_bindings = self.import_bindings(
                    importing_module,
                    import.path.semantic(),
                    NamespaceSet::ALL,
                )?;
                let source_resolved = !source_bindings.is_empty();
                let Some(name) = import.binding_name() else {
                    return Ok(ImportResolution {
                        introduced: Vec::new(),
                        source_resolved,
                    });
                };

                let mut introduced = Vec::new();
                for (namespace, source_binding) in source_bindings {
                    let Some(binding) = self.imported_binding(
                        importing_module,
                        &source_binding,
                        import.visibility,
                        ScopeBindingProvenance::NamedImport(import_ref),
                    )?
                    else {
                        continue;
                    };
                    introduced.push(ImportedScopeBinding {
                        name: name.clone(),
                        namespace,
                        binding,
                    });
                }

                Ok(ImportResolution {
                    introduced,
                    source_resolved,
                })
            }
        }
    }

    /// Build the source module scope as observed by `importing_module`.
    ///
    /// Glob imports use this to copy only bindings that pass visibility from the importer.
    pub fn visible_scope(
        &self,
        importing_module: ModuleRef,
        source_module: ModuleRef,
    ) -> Result<ModuleScopeBuilder, E::Error> {
        let mut visible_scope = ModuleScopeBuilder::default();
        for (name, entry) in self.env.module_scope_entries(source_module)? {
            for namespace in Namespace::ALL {
                for binding in entry.bindings(namespace) {
                    if self.binding_is_visible(importing_module, binding)? {
                        visible_scope.insert_binding(name, namespace, binding.clone());
                    }
                }
            }
        }

        Ok(visible_scope)
    }

    /// Returns visible macro bindings for one name without copying the whole source scope.
    pub fn visible_macro_bindings(
        &self,
        importing_module: ModuleRef,
        source_module: ModuleRef,
        name: &Name,
    ) -> Result<Vec<ScopeBinding>, E::Error> {
        let Some(entry) = self.env.module_scope_entry(source_module, name.as_str())? else {
            return Ok(Vec::new());
        };

        let mut bindings = Vec::new();
        for binding in entry.bindings(Namespace::Macros) {
            if self.binding_is_visible(importing_module, binding)? {
                bindings.push(binding.clone());
            }
        }

        Ok(bindings)
    }

    /// Returns unqualified macro binding buckets before applying macro lookup precedence.
    pub fn visible_unqualified_macro_bindings(
        &self,
        importing_module: ModuleRef,
        module_scope_modules: impl IntoIterator<Item = ModuleRef>,
        name: &Name,
    ) -> Result<UnqualifiedMacroBindings, E::Error> {
        let mut module_bindings = Vec::new();
        for module_ref in module_scope_modules {
            module_bindings.extend(self.visible_macro_bindings(
                importing_module,
                module_ref,
                name,
            )?);
        }

        Ok(UnqualifiedMacroBindings {
            module_scope: module_bindings,
            standard_prelude: self.visible_prelude_macro_bindings(importing_module, name)?,
        })
    }

    fn visible_prelude_macro_bindings(
        &self,
        importing_module: ModuleRef,
        name: &Name,
    ) -> Result<Vec<ScopeBinding>, E::Error> {
        let Some(prelude_module) = self
            .env
            .prelude_module(importing_module.origin.origin_target())?
        else {
            return Ok(Vec::new());
        };

        self.visible_macro_bindings(importing_module, prelude_module, name)
    }

    /// Resolve a normal Rust path from one module.
    ///
    /// This includes module keywords, the extern prelude, the selected standard prelude, and
    /// namespace-specific shadowing for the first segment.
    pub fn resolve_path(
        &self,
        importing_module: ModuleRef,
        path: &Path,
        terminal_filter: NamespaceSet,
    ) -> Result<ResolvePathResult, E::Error> {
        self.resolve_path_segments(
            importing_module,
            path.absolute,
            &path.segments,
            terminal_filter,
        )
    }

    /// Resolve an import path to every definition it denotes from the importing module.
    pub fn import_defs(
        &self,
        importing_module: ModuleRef,
        path: &Path,
    ) -> Result<Vec<DefId>, E::Error> {
        let mut defs = UniqueVec::new();
        for (_, binding) in self.import_bindings(importing_module, path, NamespaceSet::ALL)? {
            defs.push(binding.def);
        }
        Ok(defs.into_vec())
    }

    /// Resolve an import path and keep only module results.
    pub fn import_modules(
        &self,
        importing_module: ModuleRef,
        path: &Path,
    ) -> Result<Vec<ModuleRef>, E::Error> {
        let mut modules = UniqueVec::new();
        for (_, binding) in self.import_bindings(importing_module, path, NamespaceSet::TYPES)? {
            if let DefId::Module(module_ref) = binding.def {
                modules.push(module_ref);
            }
        }

        Ok(modules.into_vec())
    }

    /// Resolve a glob import prefix to every source that can export bindings.
    pub fn import_glob_sources(
        &self,
        importing_module: ModuleRef,
        path: &Path,
    ) -> Result<Vec<GlobImportSource>, E::Error> {
        let mut sources = UniqueVec::new();
        for (_, binding) in self.import_bindings(importing_module, path, NamespaceSet::TYPES)? {
            match binding.def {
                DefId::Module(module_ref) => {
                    sources.push(GlobImportSource::Module(module_ref));
                }
                DefId::Local(local_def_ref)
                    if self
                        .env
                        .local_def_kind(local_def_ref)?
                        .is_some_and(|kind| kind == LocalDefKind::Enum) =>
                {
                    sources.push(GlobImportSource::Enum(local_def_ref));
                }
                DefId::Local(_) | DefId::EnumVariant(_) => {}
            }
        }

        Ok(sources.into_vec())
    }

    /// Build the visible bindings exported by one glob import source.
    pub fn visible_glob_source_scope(
        &self,
        importing_module: ModuleRef,
        glob_source: GlobImportSource,
    ) -> Result<ModuleScopeBuilder, E::Error> {
        match glob_source {
            GlobImportSource::Module(source_module) => {
                self.visible_scope(importing_module, source_module)
            }
            GlobImportSource::Enum(enum_def) => {
                let mut visible_scope = ModuleScopeBuilder::default();
                for (name, namespace, binding) in
                    self.visible_enum_variant_bindings(importing_module, enum_def)?
                {
                    visible_scope.insert_binding(&name, namespace, binding);
                }
                Ok(visible_scope)
            }
        }
    }

    /// Return every namespace binding that a glob import from an enum should introduce.
    fn visible_enum_variant_bindings(
        &self,
        importing_module: ModuleRef,
        enum_def: LocalDefRef,
    ) -> Result<Vec<(Name, Namespace, ScopeBinding)>, E::Error> {
        if !self
            .env
            .local_def_kind(enum_def)?
            .is_some_and(|kind| kind == LocalDefKind::Enum)
        {
            return Ok(Vec::new());
        }

        let mut bindings = Vec::new();
        for entry in self.env.local_enum_variant_entries_for_enum(enum_def)? {
            let binding = ScopeBinding::new(
                DefId::EnumVariant(entry.variant_ref),
                entry.data.visibility,
                ScopeBindingProvenance::Direct,
            );
            if self.binding_is_visible(importing_module, &binding)? {
                for namespace in entry.data.namespaces.iter() {
                    bindings.push((entry.data.name.clone(), namespace, binding.clone()));
                }
            }
        }
        Ok(bindings)
    }

    /// Resolve a macro path by walking any prefix and reading the terminal macro bucket.
    pub fn macro_bindings(
        &self,
        importing_module: ModuleRef,
        path: &Path,
    ) -> Result<Vec<ScopeBinding>, E::Error> {
        let Some((terminal, prefix)) = path.segments.split_last() else {
            return Ok(Vec::new());
        };
        let PathSegment::Name(name) = terminal else {
            return Ok(Vec::new());
        };

        let source_modules = if prefix.is_empty() {
            if path.absolute {
                Vec::new()
            } else {
                vec![importing_module]
            }
        } else {
            self.import_modules(
                importing_module,
                &Path {
                    absolute: path.absolute,
                    segments: prefix.to_vec(),
                },
            )?
        };

        let mut bindings = Vec::new();
        for source_module in source_modules {
            let Some(entry) = self.env.module_scope_entry(source_module, name.as_str())? else {
                continue;
            };
            for binding in entry.bindings(Namespace::Macros) {
                if self.binding_is_visible(importing_module, binding)? {
                    bindings.push(binding.clone());
                }
            }
        }

        Ok(bindings)
    }

    /// Resolves an import while preserving the selected binding's semantic visibility.
    pub fn import_bindings(
        &self,
        importing_module: ModuleRef,
        path: &Path,
        terminal_filter: NamespaceSet,
    ) -> Result<Vec<(Namespace, ScopeBinding)>, E::Error> {
        let Some((terminal, prefix)) = path.segments.split_last() else {
            return Ok(Vec::new());
        };

        let PathSegment::Name(name) = terminal else {
            let resolved = self.resolve_path(importing_module, path, terminal_filter)?;
            let mut bindings = Vec::new();
            for def in resolved.resolved {
                let DefId::Module(module_ref) = def else {
                    continue;
                };
                let Some(module) = self.env.module_data(module_ref)? else {
                    continue;
                };
                for namespace in self.namespaces_for_def(def)?.iter() {
                    bindings.push((
                        namespace,
                        ScopeBinding::new(def, module.visibility, ScopeBindingProvenance::Direct),
                    ));
                }
            }
            return Ok(bindings);
        };

        if prefix.is_empty() {
            return self.first_name_bindings(
                importing_module,
                path.absolute,
                name.as_str(),
                terminal_filter,
            );
        }

        let prefix = self.resolve_path_segments(
            importing_module,
            path.absolute,
            prefix,
            NamespaceSet::TYPES,
        )?;
        let mut bindings = Vec::new();
        for def in prefix.resolved {
            match def {
                DefId::Module(module_ref) => bindings.extend(self.bindings_in_module(
                    importing_module,
                    module_ref,
                    name.as_str(),
                    terminal_filter,
                )?),
                DefId::Local(enum_def) => bindings.extend(self.enum_variant_bindings(
                    importing_module,
                    enum_def,
                    name.as_str(),
                    terminal_filter,
                )?),
                DefId::EnumVariant(_) => {}
            }
        }
        Ok(bindings)
    }

    /// Shared path walker for ordinary item paths and imports.
    ///
    /// The first segment chooses the initial search space. Each following segment is resolved inside
    /// the modules or enum definitions produced by the previous step.
    fn resolve_path_segments(
        &self,
        importing_module: ModuleRef,
        absolute: bool,
        segments: &[PathSegment],
        terminal_filter: NamespaceSet,
    ) -> Result<ResolvePathResult, E::Error> {
        let Some((first_segment, remaining_segments)) = segments.split_first() else {
            return Ok(Self::unresolved_at(0));
        };

        let mut current_defs = self.first_segment(
            importing_module,
            absolute,
            first_segment,
            NamespaceSet::for_segment(!remaining_segments.is_empty(), terminal_filter),
        )?;

        if current_defs.is_empty() {
            return Ok(Self::unresolved_at(0));
        }

        for (segment_idx, segment) in remaining_segments.iter().enumerate() {
            current_defs = self.next_segment(
                importing_module,
                current_defs,
                segment,
                NamespaceSet::for_segment(
                    segment_idx + 1 < remaining_segments.len(),
                    terminal_filter,
                ),
            )?;

            if current_defs.is_empty() {
                return Ok(Self::unresolved_at(segment_idx + 1));
            }
        }

        Ok(ResolvePathResult {
            resolved: current_defs,
            unresolved_at: None,
        })
    }

    /// Resolve the path head, where Rust allows roots, lexical lookup, and prelude fallback.
    fn first_segment(
        &self,
        importing_module: ModuleRef,
        absolute: bool,
        segment: &PathSegment,
        filter: NamespaceSet,
    ) -> Result<Vec<DefId>, E::Error> {
        if absolute {
            return match segment {
                PathSegment::Name(name) => {
                    let mut defs = UniqueVec::new();
                    for (_, binding) in
                        self.first_name_bindings(importing_module, true, name.as_str(), filter)?
                    {
                        defs.push(binding.def);
                    }
                    Ok(defs.into_vec())
                }
                PathSegment::SelfKw
                | PathSegment::SuperKw
                | PathSegment::CrateKw
                | PathSegment::DollarCrate(_) => Ok(Vec::new()),
            };
        }

        match segment {
            PathSegment::DollarCrate(target) => Ok(self
                .env
                .root_module(*target)?
                .map(DefId::Module)
                .into_iter()
                .collect()),
            PathSegment::SelfKw => Ok(vec![DefId::Module(importing_module)]),
            PathSegment::SuperKw => Ok(self
                .env
                .parent_module(importing_module)?
                .map(DefId::Module)
                .into_iter()
                .collect()),
            PathSegment::CrateKw => Ok(self
                .env
                .root_module(importing_module.origin.origin_target())?
                .map(DefId::Module)
                .into_iter()
                .collect()),
            PathSegment::Name(name) => {
                let mut defs = UniqueVec::new();
                for (_, binding) in
                    self.first_name_bindings(importing_module, false, name.as_str(), filter)?
                {
                    defs.push(binding.def);
                }
                Ok(defs.into_vec())
            }
        }
    }

    /// Resolve a segment inside the containers produced by the previous segment.
    ///
    /// Modules expose ordinary scope entries. Enum definitions expose their variant constructors in
    /// value positions.
    fn next_segment(
        &self,
        importing_module: ModuleRef,
        current_defs: Vec<DefId>,
        segment: &PathSegment,
        filter: NamespaceSet,
    ) -> Result<Vec<DefId>, E::Error> {
        let mut next_defs = UniqueVec::new();

        for current_def in current_defs {
            match current_def {
                DefId::Module(module_ref) => match segment {
                    PathSegment::SelfKw => {
                        next_defs.push(DefId::Module(module_ref));
                    }
                    PathSegment::SuperKw => {
                        if let Some(parent) = self.env.parent_module(module_ref)? {
                            next_defs.push(DefId::Module(parent));
                        }
                    }
                    PathSegment::CrateKw => {
                        if let Some(root) =
                            self.env.root_module(module_ref.origin.origin_target())?
                        {
                            next_defs.push(DefId::Module(root));
                        }
                    }
                    PathSegment::DollarCrate(_) => {}
                    PathSegment::Name(name) => {
                        for resolved_def in self.name_in_module(
                            importing_module,
                            module_ref,
                            name.as_str(),
                            filter,
                        )? {
                            next_defs.push(resolved_def);
                        }
                    }
                },
                DefId::Local(local_def_ref) => {
                    if let PathSegment::Name(name) = segment
                        && let Some(variant) =
                            self.enum_variant_for_name(local_def_ref, name.as_str(), filter)?
                    {
                        next_defs.push(DefId::EnumVariant(variant));
                    }
                }
                DefId::EnumVariant(_) => {}
            }
        }

        Ok(next_defs.into_vec())
    }

    /// Resolves one textual name inside one module scope.
    fn name_in_module(
        &self,
        importing_module: ModuleRef,
        module_ref: ModuleRef,
        name: &str,
        filter: NamespaceSet,
    ) -> Result<Vec<DefId>, E::Error> {
        let mut defs = UniqueVec::new();
        for (_, binding) in self.bindings_in_module(importing_module, module_ref, name, filter)? {
            defs.push(binding.def);
        }
        Ok(defs.into_vec())
    }

    /// Returns selected bindings for a first path segment with per-namespace fallback.
    fn first_name_bindings(
        &self,
        importing_module: ModuleRef,
        absolute: bool,
        name: &str,
        filter: NamespaceSet,
    ) -> Result<Vec<(Namespace, ScopeBinding)>, E::Error> {
        if absolute {
            if !filter.contains(Namespace::Types) {
                return Ok(Vec::new());
            }
            return Ok(self
                .env
                .extern_root(importing_module.origin.origin_target(), name)?
                .map(|module_ref| {
                    vec![(
                        Namespace::Types,
                        ScopeBinding::new(
                            DefId::Module(module_ref),
                            Visibility::Public,
                            ScopeBindingProvenance::Direct,
                        ),
                    )]
                })
                .unwrap_or_default());
        }

        // Synthetic modules are lexical scopes. A binding shadows only its own namespace, so the
        // walk keeps looking for missing namespaces in outer scopes.
        let mut bindings = Vec::new();
        let mut current = Some(importing_module);
        while let Some(module_ref) = current {
            let occupied_before = Namespace::ALL
                .map(|namespace| bindings.iter().any(|(occupied, _)| *occupied == namespace));
            for (namespace, binding) in
                self.bindings_in_module(importing_module, module_ref, name, filter)?
            {
                if !occupied_before[namespace.sort_rank() as usize] {
                    bindings.push((namespace, binding));
                }
            }

            let Some(module) = self.env.module_data(module_ref)? else {
                break;
            };
            if !matches!(module.origin, ModuleOrigin::Synthetic { .. }) {
                break;
            }
            current = self.env.parent_module(module_ref)?;
        }

        // Extern and standard preludes fill only namespaces that lexical lookup did not occupy.
        if filter.contains(Namespace::Types)
            && !bindings
                .iter()
                .any(|(namespace, _)| *namespace == Namespace::Types)
            && let Some(module_ref) = self
                .env
                .extern_root(importing_module.origin.origin_target(), name)?
        {
            bindings.push((
                Namespace::Types,
                ScopeBinding::new(
                    DefId::Module(module_ref),
                    Visibility::Public,
                    ScopeBindingProvenance::Direct,
                ),
            ));
        }

        if let Some(prelude_module) = self
            .env
            .prelude_module(importing_module.origin.origin_target())?
        {
            let occupied_before = Namespace::ALL
                .map(|namespace| bindings.iter().any(|(occupied, _)| *occupied == namespace));
            for (namespace, binding) in
                self.bindings_in_module(importing_module, prelude_module, name, filter)?
            {
                if !occupied_before[namespace.sort_rank() as usize] {
                    bindings.push((namespace, binding));
                }
            }
        }

        Ok(bindings)
    }

    fn bindings_in_module(
        &self,
        importing_module: ModuleRef,
        module_ref: ModuleRef,
        name: &str,
        filter: NamespaceSet,
    ) -> Result<Vec<(Namespace, ScopeBinding)>, E::Error> {
        let Some(entry) = self.env.module_scope_entry(module_ref, name)? else {
            return Ok(Vec::new());
        };

        let mut bindings = Vec::new();
        for namespace in filter.iter() {
            for binding in entry.bindings(namespace) {
                if self.binding_is_visible(importing_module, binding)? {
                    bindings.push((namespace, binding.clone()));
                }
            }
        }
        Ok(bindings)
    }

    fn enum_variant_bindings(
        &self,
        importing_module: ModuleRef,
        enum_def: LocalDefRef,
        name: &str,
        filter: NamespaceSet,
    ) -> Result<Vec<(Namespace, ScopeBinding)>, E::Error> {
        if !self
            .env
            .local_def_kind(enum_def)?
            .is_some_and(|kind| kind == LocalDefKind::Enum)
        {
            return Ok(Vec::new());
        }

        for entry in self.env.local_enum_variant_entries_for_enum(enum_def)? {
            if entry.data.name != name {
                continue;
            }
            let binding = ScopeBinding::new(
                DefId::EnumVariant(entry.variant_ref),
                entry.data.visibility,
                ScopeBindingProvenance::Direct,
            );
            if !self.binding_is_visible(importing_module, &binding)? {
                return Ok(Vec::new());
            }
            return Ok(entry
                .data
                .namespaces
                .iter()
                .filter(|namespace| filter.contains(*namespace))
                .map(|namespace| (namespace, binding.clone()))
                .collect());
        }
        Ok(Vec::new())
    }
}
