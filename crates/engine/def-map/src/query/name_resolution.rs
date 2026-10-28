//! Name and path lookup through DefMap item scopes.
//!
//! DefMap construction, body-local resolution, and ordinary queries all need the same
//! lookup rules while reading from different storage. This module owns those shared rules:
//! lexical/body lookup, module path lookup, import source lookup, visibility filtering, and the small
//! macro-namespace queries needed before macro-specific precedence is applied.
//!
//! The resolver does not own scopes. It reads through `NameResolutionEnv` and
//! `CrateResolutionEnv`, so construction can pass scopes under construction while frozen
//! queries read persisted DefMaps. Import scheduling, inserting answers, and forwarding glob
//! updates belong to construction; these queries only inspect the names available at the time.

use rg_ir_model::{DefId, LocalDefRef, LocalEnumVariantRef, ModuleRef, Path, PathRoot};
use rg_std::UniqueVec;
use rg_text::Name;

use super::resolution_env::{CrateResolutionEnv, NameResolutionEnv};
use crate::{
    ImportData, LocalDefKind, ModuleOrigin, NameBinding, NameBindingProvenance, NameBindingsView,
    Namespace, NamespaceSet, PerNs, Visibility,
};

/// Macro candidates kept in the buckets required by Rust lookup precedence.
///
/// Callers try `module_scope` before `standard_prelude`; this type only preserves the split while
/// sharing the visibility walk.
pub struct UnqualifiedMacroBindings {
    pub module_scope: Option<NameBinding>,
    pub standard_prelude: Option<NameBinding>,
}

/// Result of walking a path through the current scope graph.
///
/// `unresolved_at` points at the first source path component that could not be resolved. Root
/// keywords count as source components even though `Path` stores them separately from ordinary
/// name segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvePathResult {
    pub resolved: Vec<DefId>,
    pub unresolved_at: Option<usize>,
}

/// A prefix has one type-namespace target; failures retain their written segment position.
enum PrefixResolution {
    Resolved(DefId),
    UnresolvedAt(usize),
}

/// Item that can appear on the left side of a glob import.
///
/// Modules export visible item bindings. Enums export visible variant constructors into the value
/// namespace, as in `use Option::*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobImportSource {
    Module(ModuleRef),
    Enum(LocalDefRef),
}

/// Finds item names and paths through the module graph exposed by an environment.
///
/// This also owns visibility checks and unqualified macro lookup. It reads the answers available
/// in item scopes without collecting declarations or scheduling imports.
///
/// `NameResolver` is storage-agnostic on purpose. During construction the environment can expose
/// partial builders and their retained scopes; after freezing the same lookup code reads frozen
/// DefMaps through `DefMapQuery`.
pub struct NameResolver<'env, E: ?Sized> {
    env: &'env E,
}

impl<'env, E: ?Sized> NameResolver<'env, E> {
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

impl<E: NameResolutionEnv + ?Sized> NameResolver<'_, E> {
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

    /// Walk a path through lexical scopes without module-keyword or crate fallback rules.
    ///
    /// Body-local paths use this form because synthetic modules represent nested lexical scopes,
    /// not full Rust modules with extern roots and preludes.
    pub fn resolve_lexical_path(
        &self,
        importing_module: ModuleRef,
        path: &Path,
        terminal_filter: NamespaceSet,
    ) -> Result<ResolvePathResult, E::Error> {
        if !path.is_relative() {
            return Ok(Self::unresolved_at(0));
        }

        let Some((terminal, prefix)) = path.segments().split_last() else {
            return Ok(Self::unresolved_at(0));
        };

        // Only the terminal can contribute several namespaces. Prefixes select one type binding.
        let Some((first, remaining)) = prefix.split_first() else {
            let resolved = match self.first_lexical_entry(
                importing_module,
                terminal.as_str(),
                terminal_filter,
            )? {
                Some(entry) => {
                    self.visible_defs_in_entry(importing_module, entry, terminal_filter)?
                }
                None => Vec::new(),
            };
            return Ok(if resolved.is_empty() {
                Self::unresolved_at(0)
            } else {
                ResolvePathResult {
                    resolved,
                    unresolved_at: None,
                }
            });
        };
        let Some(entry) =
            self.first_lexical_entry(importing_module, first.as_str(), NamespaceSet::TYPES)?
        else {
            return Ok(Self::unresolved_at(0));
        };
        let first = entry
            .binding(Namespace::Types)
            .expect("type lookup selected a type binding")
            .def;
        match self.walk_prefix(importing_module, first, remaining, 1)? {
            PrefixResolution::Resolved(container) => self.resolve_terminal(
                importing_module,
                container,
                terminal.as_str(),
                terminal_filter,
                path.segments().len() - 1,
            ),
            PrefixResolution::UnresolvedAt(segment) => Ok(Self::unresolved_at(segment)),
        }
    }

    /// Resolve one name inside a single lexical module without walking parent scopes.
    pub fn resolve_lexical_name_in_module(
        &self,
        importing_module: ModuleRef,
        module_ref: ModuleRef,
        name: &str,
        filter: NamespaceSet,
    ) -> Result<Vec<DefId>, E::Error> {
        let Some(entry) = self.env.module_scope_entry(module_ref, name)? else {
            return Ok(Vec::new());
        };
        self.visible_defs_in_entry(importing_module, entry, filter)
    }

    /// Several namespaces can expose the same definition, so terminal answers remain deduplicated.
    fn visible_defs_in_entry(
        &self,
        importing_module: ModuleRef,
        entry: NameBindingsView<'_>,
        filter: NamespaceSet,
    ) -> Result<Vec<DefId>, E::Error> {
        let mut defs = UniqueVec::new();
        for namespace in filter.iter() {
            if let Some(binding) = entry.binding(namespace)
                && self.binding_is_visible(importing_module, binding)?
            {
                defs.push(binding.def);
            }
        }
        Ok(defs.into_vec())
    }

    /// Follow one type binding per prefix segment, preserving the first failed position.
    fn walk_prefix(
        &self,
        importing_module: ModuleRef,
        mut container: DefId,
        segments: &[Name],
        first_segment: usize,
    ) -> Result<PrefixResolution, E::Error> {
        for (offset, segment) in segments.iter().enumerate() {
            let next = match container {
                DefId::Module(module) => self
                    .binding_in_module(
                        importing_module,
                        module,
                        segment.as_str(),
                        Namespace::Types,
                    )?
                    .map(|binding| binding.def),
                DefId::Local(def) => self
                    .enum_variant_for_name(def, segment.as_str(), NamespaceSet::TYPES)?
                    .map(DefId::EnumVariant),
                DefId::EnumVariant(_) => None,
            };
            let Some(next) = next else {
                return Ok(PrefixResolution::UnresolvedAt(first_segment + offset));
            };
            container = next;
        }
        Ok(PrefixResolution::Resolved(container))
    }

    /// The last name may occupy type, value, and macro slots inside its single container.
    fn resolve_terminal(
        &self,
        importing_module: ModuleRef,
        container: DefId,
        name: &str,
        filter: NamespaceSet,
        segment: usize,
    ) -> Result<ResolvePathResult, E::Error> {
        let resolved = match container {
            DefId::Module(module) => {
                self.resolve_lexical_name_in_module(importing_module, module, name, filter)?
            }
            DefId::Local(def) => self
                .enum_variant_for_name(def, name, filter)?
                .map(DefId::EnumVariant)
                .into_iter()
                .collect(),
            DefId::EnumVariant(_) => Vec::new(),
        };
        Ok(if resolved.is_empty() {
            Self::unresolved_at(segment)
        } else {
            ResolvePathResult {
                resolved,
                unresolved_at: None,
            }
        })
    }

    fn binding_in_module(
        &self,
        importing_module: ModuleRef,
        module: ModuleRef,
        name: &str,
        namespace: Namespace,
    ) -> Result<Option<NameBinding>, E::Error> {
        let Some(binding) = self
            .env
            .module_scope_entry(module, name)?
            .and_then(|entry| entry.binding(namespace))
            .copied()
        else {
            return Ok(None);
        };
        Ok(self
            .binding_is_visible(importing_module, &binding)?
            .then_some(binding))
    }

    fn enum_variant_for_name(
        &self,
        enum_def: LocalDefRef,
        name: &str,
        filter: NamespaceSet,
    ) -> Result<Option<LocalEnumVariantRef>, E::Error> {
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

    /// Find the first lexical scope with a visible binding in the requested namespaces.
    fn first_lexical_entry(
        &self,
        importing_module: ModuleRef,
        name: &str,
        filter: NamespaceSet,
    ) -> Result<Option<NameBindingsView<'_>>, E::Error> {
        let mut current = Some(importing_module);
        while let Some(module_ref) = current {
            if let Some(entry) = self.env.module_scope_entry(module_ref, name)? {
                for namespace in filter.iter() {
                    if let Some(binding) = entry.binding(namespace)
                        && self.binding_is_visible(importing_module, binding)?
                    {
                        return Ok(Some(entry));
                    }
                }
            }
            let Some(module) = self.env.module_metadata(module_ref)? else {
                break;
            };
            if !matches!(module.origin, ModuleOrigin::Synthetic { .. }) {
                break;
            }
            current = self.env.parent_module(module_ref)?;
        }
        Ok(None)
    }

    /// Check whether the selected binding is visible from a module.
    pub fn binding_is_visible(
        &self,
        importing_module: ModuleRef,
        binding: &NameBinding,
    ) -> Result<bool, E::Error> {
        self.visibility_is_visible(importing_module, binding.visibility)
    }

    /// Apply the use item's visibility without widening the source definition.
    ///
    /// A direct `macro_rules!` definition may be re-exported through its crate, and an exported
    /// definition may be public. Imported aliases use the visibility already selected for them.
    pub fn imported_binding(
        &self,
        importing_module: ModuleRef,
        source: &NameBinding,
        import: &ImportData,
        provenance: NameBindingProvenance,
    ) -> Result<Option<NameBinding>, E::Error> {
        if !self.binding_is_visible(importing_module, source)? {
            return Ok(None);
        }
        let source_visibility = if source.is_exported_macro() {
            Visibility::Public
        } else if source.has_direct_macro_rules() {
            self.local_def_crate_visibility(source.def)?
                .unwrap_or(source.visibility)
        } else {
            source.visibility
        };
        Ok(self
            .intersect_visibility(source_visibility, import.visibility)?
            .map(|visibility| NameBinding::from_import(source.def, visibility, provenance, import)))
    }

    pub(crate) fn broadest_visibility(
        &self,
        left: Visibility,
        right: Visibility,
    ) -> Result<Visibility, E::Error> {
        Ok(match (left, right) {
            (Visibility::Public, _) | (_, Visibility::Public) => Visibility::Public,
            (Visibility::Invisible, visibility) | (visibility, Visibility::Invisible) => visibility,
            (Visibility::Module(left), Visibility::Module(right)) => {
                // Valid restrictions are nested ancestors of the importing module. Incomparable
                // restrictions from incomplete code keep the first region.
                if self.module_is_descendant_of(left, right)? {
                    Visibility::Module(right)
                } else {
                    Visibility::Module(left)
                }
            }
        })
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
                .module_metadata(ModuleRef {
                    origin: module.origin,
                    module: module_id,
                })?
                .and_then(|module| module.parent);
        }

        Ok(false)
    }
}

impl<E: CrateResolutionEnv + ?Sized> NameResolver<'_, E> {
    /// Visit visible selected bindings without copying them into construction storage.
    pub(crate) fn visit_visible_bindings(
        &self,
        importing_module: ModuleRef,
        source_module: ModuleRef,
        filter: NamespaceSet,
        mut visit: impl FnMut(&Name, Namespace, &NameBinding) -> Result<(), E::Error>,
    ) -> Result<(), E::Error> {
        for (name, entry) in self.env.module_scope_entries(source_module)? {
            for namespace in filter.iter() {
                if let Some(binding) = entry.binding(namespace)
                    && self.binding_is_visible(importing_module, binding)?
                {
                    visit(name, namespace, binding)?;
                }
            }
        }
        Ok(())
    }

    /// Return the selected macro binding when it is visible to the caller.
    pub fn visible_macro_binding(
        &self,
        importing_module: ModuleRef,
        source_module: ModuleRef,
        name: &Name,
    ) -> Result<Option<NameBinding>, E::Error> {
        self.binding_in_module(importing_module, source_module, name, Namespace::Macros)
    }

    /// Keep module and prelude lookup separate so item callers can filter textual macro order
    /// before trying the prelude. The first visible module wins along a lexical parent chain.
    pub fn visible_unqualified_macro_bindings(
        &self,
        importing_module: ModuleRef,
        module_scope_modules: impl IntoIterator<Item = ModuleRef>,
        name: &Name,
    ) -> Result<UnqualifiedMacroBindings, E::Error> {
        let mut module_scope = None;
        for module in module_scope_modules {
            module_scope = self.visible_macro_binding(importing_module, module, name)?;
            if module_scope.is_some() {
                break;
            }
        }
        let standard_prelude = match self
            .env
            .prelude_module(importing_module.origin.origin_crate())?
        {
            Some(prelude) => self.visible_macro_binding(importing_module, prelude, name)?,
            None => None,
        };
        Ok(UnqualifiedMacroBindings {
            module_scope,
            standard_prelude,
        })
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
            path.root(),
            path.segments(),
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

    /// Resolve the type namespace of an import path and keep a module target.
    pub fn import_module(
        &self,
        importing_module: ModuleRef,
        path: &Path,
    ) -> Result<Option<ModuleRef>, E::Error> {
        Ok(
            match self.resolve_prefix(importing_module, path.root(), path.segments())? {
                PrefixResolution::Resolved(DefId::Module(module)) => Some(module),
                _ => None,
            },
        )
    }

    /// A glob source is one selected module or enum in the type namespace.
    pub fn import_glob_source(
        &self,
        importing_module: ModuleRef,
        path: &Path,
    ) -> Result<Option<GlobImportSource>, E::Error> {
        let PrefixResolution::Resolved(def) =
            self.resolve_prefix(importing_module, path.root(), path.segments())?
        else {
            return Ok(None);
        };
        Ok(match def {
            DefId::Module(module) => Some(GlobImportSource::Module(module)),
            DefId::Local(local) if self.env.local_def_kind(local)? == Some(LocalDefKind::Enum) => {
                Some(GlobImportSource::Enum(local))
            }
            _ => None,
        })
    }

    /// Resolve a macro path through its selected module prefix and macro namespace binding.
    pub fn macro_binding(
        &self,
        importing_module: ModuleRef,
        path: &Path,
    ) -> Result<Option<NameBinding>, E::Error> {
        let Some((terminal, prefix)) = path.segments().split_last() else {
            return Ok(None);
        };
        let source_module = if prefix.is_empty() {
            match path.root() {
                PathRoot::Relative => Some(importing_module),
                PathRoot::Absolute => None,
                PathRoot::Crate
                | PathRoot::SelfModule
                | PathRoot::Super(_)
                | PathRoot::DollarCrate(_) => {
                    self.import_module(importing_module, &Path::new(path.root(), Vec::new()))?
                }
            }
        } else {
            self.import_module(importing_module, &Path::new(path.root(), prefix.to_vec()))?
        };
        match source_module {
            Some(module) => self.visible_macro_binding(importing_module, module, terminal),
            None => Ok(None),
        }
    }

    /// Resolves an import while preserving the selected binding's semantic visibility.
    pub fn import_bindings(
        &self,
        importing_module: ModuleRef,
        path: &Path,
        terminal_filter: NamespaceSet,
    ) -> Result<Vec<(Namespace, NameBinding)>, E::Error> {
        let Some((terminal, prefix)) = path.segments().split_last() else {
            let resolved = self.resolve_path(importing_module, path, terminal_filter)?;
            let mut bindings = Vec::new();
            for def in resolved.resolved {
                let DefId::Module(module_ref) = def else {
                    continue;
                };
                let Some(module) = self.env.module_metadata(module_ref)? else {
                    continue;
                };
                for namespace in self.namespaces_for_def(def)?.iter() {
                    bindings.push((
                        namespace,
                        NameBinding::new(def, module.visibility, NameBindingProvenance::Direct),
                    ));
                }
            }
            return Ok(bindings);
        };

        if prefix.is_empty() && matches!(path.root(), PathRoot::Relative | PathRoot::Absolute) {
            let bindings = self.first_name_bindings(
                importing_module,
                path.is_absolute(),
                terminal.as_str(),
                terminal_filter,
            )?;
            return Ok(terminal_filter
                .iter()
                .filter_map(|namespace| bindings.get(namespace).map(|binding| (namespace, binding)))
                .collect());
        }

        let PrefixResolution::Resolved(prefix) =
            self.resolve_prefix(importing_module, path.root(), prefix)?
        else {
            return Ok(Vec::new());
        };
        match prefix {
            DefId::Module(module) => self.bindings_in_module(
                importing_module,
                module,
                terminal.as_str(),
                terminal_filter,
            ),
            DefId::Local(def) => self.enum_variant_bindings(
                importing_module,
                def,
                terminal.as_str(),
                terminal_filter,
            ),
            DefId::EnumVariant(_) => Ok(Vec::new()),
        }
    }

    /// Resolve the prefix once, then collect the terminal's selected namespace results.
    fn resolve_path_segments(
        &self,
        importing_module: ModuleRef,
        root: PathRoot,
        segments: &[Name],
        terminal_filter: NamespaceSet,
    ) -> Result<ResolvePathResult, E::Error> {
        let Some((terminal, prefix)) = segments.split_last() else {
            let module = if terminal_filter.contains(Namespace::Types) {
                self.root_module(importing_module, root)?
            } else {
                None
            };
            return Ok(match module {
                Some(module) => ResolvePathResult {
                    resolved: vec![DefId::Module(module)],
                    unresolved_at: None,
                },
                None => Self::unresolved_at(0),
            });
        };

        if prefix.is_empty() && matches!(root, PathRoot::Relative | PathRoot::Absolute) {
            let bindings = self.first_name_bindings(
                importing_module,
                root.is_absolute(),
                terminal.as_str(),
                terminal_filter,
            )?;
            let mut defs = UniqueVec::new();
            for namespace in terminal_filter.iter() {
                if let Some(binding) = bindings.get(namespace) {
                    defs.push(binding.def);
                }
            }
            return Ok(if defs.is_empty() {
                Self::unresolved_at(0)
            } else {
                ResolvePathResult {
                    resolved: defs.into_vec(),
                    unresolved_at: None,
                }
            });
        }

        match self.resolve_prefix(importing_module, root, prefix)? {
            PrefixResolution::Resolved(container) => self.resolve_terminal(
                importing_module,
                container,
                terminal.as_str(),
                terminal_filter,
                root.written_component_count() + segments.len() - 1,
            ),
            PrefixResolution::UnresolvedAt(segment) => Ok(Self::unresolved_at(segment)),
        }
    }

    fn resolve_prefix(
        &self,
        importing_module: ModuleRef,
        root: PathRoot,
        segments: &[Name],
    ) -> Result<PrefixResolution, E::Error> {
        let (first, remaining, first_segment) = match root {
            PathRoot::Relative | PathRoot::Absolute => {
                let Some((first, remaining)) = segments.split_first() else {
                    return Ok(PrefixResolution::UnresolvedAt(0));
                };
                let first = self
                    .first_name_bindings(
                        importing_module,
                        root.is_absolute(),
                        first.as_str(),
                        NamespaceSet::TYPES,
                    )?
                    .get(Namespace::Types)
                    .map(|binding| binding.def);
                (first, remaining, 1)
            }
            _ => (
                self.root_module(importing_module, root)?.map(DefId::Module),
                segments,
                root.written_component_count(),
            ),
        };
        match first {
            Some(first) => self.walk_prefix(importing_module, first, remaining, first_segment),
            None => Ok(PrefixResolution::UnresolvedAt(0)),
        }
    }

    /// Resolve a structured keyword or macro-hygiene root to its starting module.
    fn root_module(
        &self,
        importing_module: ModuleRef,
        root: PathRoot,
    ) -> Result<Option<ModuleRef>, E::Error> {
        match root {
            PathRoot::DollarCrate(crate_ref) => self.env.root_module(crate_ref),
            PathRoot::SelfModule => Ok(Some(importing_module)),
            PathRoot::Super(depth) => {
                let mut module = Some(importing_module);
                for _ in 0..depth {
                    module = match module {
                        Some(module) => self.env.parent_module(module)?,
                        None => break,
                    };
                }
                Ok(module)
            }
            PathRoot::Crate => self.env.root_module(importing_module.origin.origin_crate()),
            PathRoot::Relative | PathRoot::Absolute => Ok(None),
        }
    }

    /// Fill one temporary slot per namespace, following lexical, extern, and prelude precedence.
    /// Stored scope entries remain sparse; this result only lives during one lookup.
    fn first_name_bindings(
        &self,
        importing_module: ModuleRef,
        absolute: bool,
        name: &str,
        filter: NamespaceSet,
    ) -> Result<PerNs<Option<NameBinding>>, E::Error> {
        let mut bindings = PerNs::default();
        if absolute {
            if filter.contains(Namespace::Types) {
                *bindings.get_mut(Namespace::Types) = self
                    .env
                    .extern_root(importing_module.origin.origin_crate(), name)?
                    .map(|module| {
                        NameBinding::new(
                            DefId::Module(module),
                            Visibility::Public,
                            NameBindingProvenance::Direct,
                        )
                    });
            }
            return Ok(bindings);
        }

        // Synthetic scopes shadow one namespace at a time. Outer scopes can fill the other slots.
        let mut current = Some(importing_module);
        while let Some(module_ref) = current {
            for (namespace, binding) in
                self.bindings_in_module(importing_module, module_ref, name, filter)?
            {
                bindings.get_mut(namespace).get_or_insert(binding);
            }
            let Some(module) = self.env.module_metadata(module_ref)? else {
                break;
            };
            if !matches!(module.origin, ModuleOrigin::Synthetic { .. }) {
                break;
            }
            current = self.env.parent_module(module_ref)?;
        }

        // Extern and standard preludes fill only slots that lexical lookup left empty.
        if filter.contains(Namespace::Types)
            && bindings.get(Namespace::Types).is_none()
            && let Some(module) = self
                .env
                .extern_root(importing_module.origin.origin_crate(), name)?
        {
            *bindings.get_mut(Namespace::Types) = Some(NameBinding::new(
                DefId::Module(module),
                Visibility::Public,
                NameBindingProvenance::Direct,
            ));
        }
        if let Some(prelude) = self
            .env
            .prelude_module(importing_module.origin.origin_crate())?
        {
            for (namespace, binding) in
                self.bindings_in_module(importing_module, prelude, name, filter)?
            {
                bindings.get_mut(namespace).get_or_insert(binding);
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
    ) -> Result<Vec<(Namespace, NameBinding)>, E::Error> {
        let Some(entry) = self.env.module_scope_entry(module_ref, name)? else {
            return Ok(Vec::new());
        };

        let mut bindings = Vec::new();
        for namespace in filter.iter() {
            if let Some(binding) = entry.binding(namespace)
                && self.binding_is_visible(importing_module, binding)?
            {
                bindings.push((namespace, *binding));
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
    ) -> Result<Vec<(Namespace, NameBinding)>, E::Error> {
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
            let binding = NameBinding::new(
                DefId::EnumVariant(entry.variant_ref),
                entry.data.visibility,
                NameBindingProvenance::Direct,
            );
            if !self.binding_is_visible(importing_module, &binding)? {
                return Ok(Vec::new());
            }
            return Ok(entry
                .data
                .namespaces
                .iter()
                .filter(|namespace| filter.contains(*namespace))
                .map(|namespace| (namespace, binding))
                .collect());
        }
        Ok(Vec::new())
    }
}
