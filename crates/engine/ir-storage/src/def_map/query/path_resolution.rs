//! Helpers for resolving paths against def-map scopes.
//!
//! Resolution here is intentionally narrow:
//! - it works only with already-built module scopes
//! - it understands module navigation (`self`, `super`, `crate`)
//! - it can return multiple definitions because several namespaces may share one textual name
//!
//! During def-map construction this module reads from the fixed-point scope snapshot. After
//! construction, the same path-walking logic reads from frozen `DefMapDb` data.

use rg_ir_model::items::VisibilityLevel;
use rg_ir_model::{
    DefId, DefMapRef, LocalDefRef, ModuleId, ModuleRef, Path, PathSegment, TargetRef,
};
use rg_std::UniqueVec;
use rg_text::Name;

use super::super::{
    ImportPath, LocalDefKind, ModuleOrigin, ModuleScopeBuilder, Namespace, ScopeBinding,
};

use super::resolution_env::{ScopeResolutionEnv, TargetResolutionEnv};

/// Privacy-visible macro bindings collected before Rust macro lookup precedence is applied.
pub struct UnqualifiedMacroBindings {
    pub module_scope: Vec<ScopeBinding>,
    pub standard_prelude: Vec<ScopeBinding>,
}

/// Result of resolving a path against the frozen def-map graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvePathResult {
    pub resolved: Vec<DefId>,
    pub unresolved_at: Option<usize>,
}

/// Source accepted by a glob import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobImportSource {
    Module(ModuleRef),
    Enum(LocalDefRef),
}

/// Namespace buckets that may contribute definitions for a path terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameResolutionFilter {
    /// Type, value, and macro namespaces.
    AllNamespaces,
    /// Type namespace only; used for path prefixes and type positions.
    TypesOnly,
    /// Value namespace only; used by expression/value path lookup.
    ValuesOnly,
}

impl NameResolutionFilter {
    fn for_segment(path_prefix: bool, terminal_filter: Self) -> Self {
        if path_prefix {
            Self::TypesOnly
        } else {
            terminal_filter
        }
    }
}

/// Groups path lookup operations around one scope source.
pub struct PathResolver<'env, E: ?Sized> {
    env: &'env E,
}

impl<'env, E: ?Sized> PathResolver<'env, E> {
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

impl<E: ScopeResolutionEnv + ?Sized> PathResolver<'_, E> {
    pub fn namespace_for_def(&self, def: DefId) -> Result<Option<Namespace>, E::Error> {
        match def {
            DefId::Module(_) => Ok(Some(Namespace::Types)),
            DefId::Local(local_def_ref) => Ok(self
                .env
                .local_def_kind(local_def_ref)?
                .map(|kind| kind.namespace())),
            DefId::EnumVariant(_) => Ok(Some(Namespace::Values)),
        }
    }

    /// Walks a path through lexical scopes without module-keyword or target fallback rules.
    pub fn resolve_lexical_path(
        &self,
        importing_module: ModuleRef,
        path: &Path,
        terminal_filter: NameResolutionFilter,
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
            NameResolutionFilter::for_segment(!remaining_segments.is_empty(), terminal_filter),
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
                NameResolutionFilter::for_segment(
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

    pub fn resolve_lexical_name_in_module(
        &self,
        importing_module: ModuleRef,
        module_ref: ModuleRef,
        name: &str,
        filter: NameResolutionFilter,
    ) -> Result<Vec<DefId>, E::Error> {
        let Some(scope_entry) = self.env.module_scope_entry(module_ref, name)? else {
            return Ok(Vec::new());
        };

        let mut defs = UniqueVec::new();
        if !matches!(filter, NameResolutionFilter::ValuesOnly) {
            for binding in scope_entry.types() {
                if self.lexical_binding_is_visible(importing_module, binding)? {
                    defs.push(binding.def);
                }
            }
        }

        if matches!(filter, NameResolutionFilter::TypesOnly) {
            return Ok(defs.into_vec());
        }

        for binding in scope_entry.values() {
            if self.lexical_binding_is_visible(importing_module, binding)? {
                defs.push(binding.def);
            }
        }

        if matches!(filter, NameResolutionFilter::ValuesOnly) {
            return Ok(defs.into_vec());
        }

        for binding in scope_entry.macros() {
            if self.lexical_binding_is_visible(importing_module, binding)? {
                defs.push(binding.def);
            }
        }

        Ok(defs.into_vec())
    }

    fn lexical_next_name_segment(
        &self,
        importing_module: ModuleRef,
        current_defs: Vec<DefId>,
        name: &str,
        filter: NameResolutionFilter,
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
        filter: NameResolutionFilter,
    ) -> Result<Option<rg_ir_model::LocalEnumVariantRef>, E::Error> {
        if matches!(filter, NameResolutionFilter::TypesOnly) {
            return Ok(None);
        }
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
            .find_map(|entry| (entry.data.name == name).then_some(entry.variant_ref)))
    }

    fn first_name_in_lexical_scope(
        &self,
        importing_module: ModuleRef,
        name: &str,
        filter: NameResolutionFilter,
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

    fn lexical_binding_is_visible(
        &self,
        importing_module: ModuleRef,
        binding: &ScopeBinding,
    ) -> Result<bool, E::Error> {
        if matches!(binding.visibility, VisibilityLevel::Public) {
            return Ok(true);
        }
        if importing_module.origin != binding.owner.origin {
            return Ok(false);
        }

        Ok(match &binding.visibility {
            VisibilityLevel::Private | VisibilityLevel::Self_ => {
                self.module_is_descendant_of(importing_module, binding.owner)?
            }
            VisibilityLevel::Crate => true,
            VisibilityLevel::Super => match self.env.parent_module(binding.owner)? {
                Some(visible_from) => {
                    self.module_is_descendant_of(importing_module, visible_from)?
                }
                None => false,
            },
            VisibilityLevel::Public => true,
            VisibilityLevel::Restricted(_) | VisibilityLevel::Unknown(_) => false,
        })
    }

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

impl<E: TargetResolutionEnv + ?Sized> PathResolver<'_, E> {
    pub fn visible_scope(
        &self,
        importing_module: ModuleRef,
        source_module: ModuleRef,
    ) -> Result<ModuleScopeBuilder, E::Error> {
        let mut visible_scope = ModuleScopeBuilder::default();
        for (name, entry) in self.env.module_scope_entries(source_module)? {
            for binding in entry.types() {
                if self.binding_is_visible(importing_module, binding)? {
                    visible_scope.insert_binding(name, Namespace::Types, binding.clone());
                }
            }

            for binding in entry.values() {
                if self.binding_is_visible(importing_module, binding)? {
                    visible_scope.insert_binding(name, Namespace::Values, binding.clone());
                }
            }

            for binding in entry.macros() {
                if self.binding_is_visible(importing_module, binding)? {
                    visible_scope.insert_binding(name, Namespace::Macros, binding.clone());
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
        for binding in entry.macros() {
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

    pub fn resolve_path(
        &self,
        importing_module: ModuleRef,
        path: &Path,
        terminal_filter: NameResolutionFilter,
    ) -> Result<ResolvePathResult, E::Error> {
        self.resolve_path_segments(
            importing_module,
            path.absolute,
            &path.segments,
            terminal_filter,
        )
    }

    /// Resolves an import path to every definition it denotes in the current scope snapshot.
    pub fn import_defs(
        &self,
        importing_target: TargetRef,
        importing_module: ModuleId,
        path: &ImportPath,
    ) -> Result<Vec<DefId>, E::Error> {
        self.import_defs_from_module(
            ModuleRef {
                origin: DefMapRef::Target(importing_target),
                module: importing_module,
            },
            path,
        )
    }

    /// Resolves a path and keeps only module results.
    pub fn import_modules(
        &self,
        importing_target: TargetRef,
        importing_module: ModuleId,
        path: &ImportPath,
    ) -> Result<Vec<ModuleRef>, E::Error> {
        self.import_modules_from_module(
            ModuleRef {
                origin: DefMapRef::Target(importing_target),
                module: importing_module,
            },
            path,
        )
    }

    /// Resolves a glob import prefix to modules and enums that can export bindings.
    pub fn import_glob_sources(
        &self,
        importing_target: TargetRef,
        importing_module: ModuleId,
        path: &ImportPath,
    ) -> Result<Vec<GlobImportSource>, E::Error> {
        self.import_glob_sources_from_module(
            ModuleRef {
                origin: DefMapRef::Target(importing_target),
                module: importing_module,
            },
            path,
        )
    }

    /// Resolves an import path from a concrete module origin.
    ///
    /// Target-level imports delegate here, and body-local import finalization can use the same path
    /// rules without losing the body origin of the importing synthetic module.
    pub fn import_defs_from_module(
        &self,
        importing_module: ModuleRef,
        path: &ImportPath,
    ) -> Result<Vec<DefId>, E::Error> {
        self.import_defs_from_module_with_filter(
            importing_module,
            path,
            NameResolutionFilter::AllNamespaces,
        )
    }

    /// Resolves an import path from a concrete module origin and keeps only module results.
    pub fn import_modules_from_module(
        &self,
        importing_module: ModuleRef,
        path: &ImportPath,
    ) -> Result<Vec<ModuleRef>, E::Error> {
        let resolved_defs = self.import_defs_from_module_with_filter(
            importing_module,
            path,
            NameResolutionFilter::TypesOnly,
        )?;

        let mut modules = UniqueVec::new();
        for resolved_def in resolved_defs {
            if let DefId::Module(module_ref) = resolved_def {
                modules.push(module_ref);
            }
        }

        Ok(modules.into_vec())
    }

    /// Resolves an import path from a concrete module origin and keeps glob-capable sources.
    pub fn import_glob_sources_from_module(
        &self,
        importing_module: ModuleRef,
        path: &ImportPath,
    ) -> Result<Vec<GlobImportSource>, E::Error> {
        let resolved_defs = self.import_defs_from_module_with_filter(
            importing_module,
            path,
            NameResolutionFilter::TypesOnly,
        )?;

        let mut sources = UniqueVec::new();
        for resolved_def in resolved_defs {
            match resolved_def {
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

    /// Returns value bindings that a glob import from an enum should introduce.
    pub fn visible_enum_variant_bindings(
        &self,
        importing_module: ModuleRef,
        enum_def: LocalDefRef,
    ) -> Result<Vec<(Name, ScopeBinding)>, E::Error> {
        let Some(enum_data) = self.env.local_def_data(enum_def)? else {
            return Ok(Vec::new());
        };
        if enum_data.kind != LocalDefKind::Enum {
            return Ok(Vec::new());
        }

        let enum_binding = ScopeBinding {
            def: DefId::Local(enum_def),
            visibility: enum_data.visibility.clone(),
            owner: ModuleRef {
                origin: enum_def.origin,
                module: enum_data.module,
            },
            origin: super::super::ScopeBindingOrigin::Direct,
        };
        if !self.binding_is_visible(importing_module, &enum_binding)? {
            return Ok(Vec::new());
        }

        Ok(self
            .env
            .local_enum_variant_entries_for_enum(enum_def)?
            .into_iter()
            .map(|entry| {
                (
                    entry.data.name.clone(),
                    ScopeBinding {
                        def: DefId::EnumVariant(entry.variant_ref),
                        visibility: entry.data.visibility.clone(),
                        owner: ModuleRef {
                            origin: enum_def.origin,
                            module: entry.data.module,
                        },
                        origin: super::super::ScopeBindingOrigin::Direct,
                    },
                )
            })
            .collect())
    }

    /// Resolves a path whose terminal segment must be a macro binding.
    pub fn macro_bindings(
        &self,
        importing_target: TargetRef,
        importing_module: ModuleId,
        path: &ImportPath,
    ) -> Result<Vec<ScopeBinding>, E::Error> {
        let Some((terminal, prefix)) = path.segments.split_last() else {
            return Ok(Vec::new());
        };
        let PathSegment::Name(name) = terminal else {
            return Ok(Vec::new());
        };

        let importing_module_ref = ModuleRef {
            origin: DefMapRef::Target(importing_target),
            module: importing_module,
        };
        let source_modules = if prefix.is_empty() {
            if path.absolute {
                Vec::new()
            } else {
                vec![importing_module_ref]
            }
        } else {
            self.import_modules(
                importing_target,
                importing_module,
                &ImportPath {
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
            for binding in entry.macros() {
                if self.binding_is_visible(importing_module_ref, binding)? {
                    bindings.push(binding.clone());
                }
            }
        }

        Ok(bindings)
    }

    fn import_defs_from_module_with_filter(
        &self,
        importing_module: ModuleRef,
        path: &ImportPath,
        terminal_filter: NameResolutionFilter,
    ) -> Result<Vec<DefId>, E::Error> {
        let result = self.resolve_path_segments(
            importing_module,
            path.absolute,
            &path.segments,
            terminal_filter,
        )?;

        Ok(result.resolved)
    }

    /// Walks a path through one target-aware resolution environment.
    fn resolve_path_segments(
        &self,
        importing_module: ModuleRef,
        absolute: bool,
        segments: &[PathSegment],
        terminal_filter: NameResolutionFilter,
    ) -> Result<ResolvePathResult, E::Error> {
        let Some((first_segment, remaining_segments)) = segments.split_first() else {
            return Ok(Self::unresolved_at(0));
        };

        let mut current_defs = self.first_segment(
            importing_module,
            absolute,
            first_segment,
            NameResolutionFilter::for_segment(!remaining_segments.is_empty(), terminal_filter),
        )?;

        if current_defs.is_empty() {
            return Ok(Self::unresolved_at(0));
        }

        for (segment_idx, segment) in remaining_segments.iter().enumerate() {
            current_defs = self.next_segment(
                importing_module,
                current_defs,
                segment,
                NameResolutionFilter::for_segment(
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

    /// Resolves the first path segment, which decides the starting search space.
    fn first_segment(
        &self,
        importing_module: ModuleRef,
        absolute: bool,
        segment: &PathSegment,
        filter: NameResolutionFilter,
    ) -> Result<Vec<DefId>, E::Error> {
        if absolute {
            return match segment {
                PathSegment::Name(name) => Ok(self
                    .env
                    .extern_root(importing_module.origin.origin_target(), name.as_str())?
                    .map(|module_ref| vec![DefId::Module(module_ref)])
                    .unwrap_or_default()),
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
                // Shadowing is namespace-specific. Prefixes and type-position terminals walk the
                // type namespace, so same-spelling value/macro bindings do not block fallback.
                let local_defs =
                    self.first_name_in_target_scope(importing_module, name.as_str(), filter)?;
                if !local_defs.is_empty() {
                    return Ok(local_defs);
                }

                if let Some(module_ref) = self
                    .env
                    .extern_root(importing_module.origin.origin_target(), name.as_str())?
                {
                    return Ok(vec![DefId::Module(module_ref)]);
                }

                let Some(prelude_module) = self
                    .env
                    .prelude_module(importing_module.origin.origin_target())?
                else {
                    return Ok(Vec::new());
                };

                self.name_in_module(importing_module, prelude_module, name.as_str(), filter)
            }
        }
    }

    /// Resolves every path segment after the first one.
    fn next_segment(
        &self,
        importing_module: ModuleRef,
        current_defs: Vec<DefId>,
        segment: &PathSegment,
        filter: NameResolutionFilter,
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
        filter: NameResolutionFilter,
    ) -> Result<Vec<DefId>, E::Error> {
        let Some(scope_entry) = self.env.module_scope_entry(module_ref, name)? else {
            return Ok(Vec::new());
        };

        let mut defs = UniqueVec::new();

        // One textual name can contribute bindings from several namespaces, so we collect them all
        // into a deduplicated result set.
        if !matches!(filter, NameResolutionFilter::ValuesOnly) {
            for binding in scope_entry.types() {
                if self.binding_is_visible(importing_module, binding)? {
                    defs.push(binding.def);
                }
            }
        }

        if matches!(filter, NameResolutionFilter::TypesOnly) {
            return Ok(defs.into_vec());
        }

        for binding in scope_entry.values() {
            if self.binding_is_visible(importing_module, binding)? {
                defs.push(binding.def);
            }
        }

        if matches!(filter, NameResolutionFilter::ValuesOnly) {
            return Ok(defs.into_vec());
        }

        for binding in scope_entry.macros() {
            if self.binding_is_visible(importing_module, binding)? {
                defs.push(binding.def);
            }
        }

        Ok(defs.into_vec())
    }

    /// Synthetic modules model lexical scopes, so unqualified lookup climbs synthetic parents until
    /// it reaches the first real module boundary.
    fn first_name_in_target_scope(
        &self,
        importing_module: ModuleRef,
        name: &str,
        filter: NameResolutionFilter,
    ) -> Result<Vec<DefId>, E::Error> {
        let mut current = Some(importing_module);
        while let Some(module_ref) = current {
            let defs = self.name_in_module(importing_module, module_ref, name, filter)?;
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

    /// Checks whether a binding can be observed from the importing module.
    fn binding_is_visible(
        &self,
        importing_module: ModuleRef,
        binding: &ScopeBinding,
    ) -> Result<bool, E::Error> {
        if matches!(binding.visibility, VisibilityLevel::Public) {
            return Ok(true);
        }

        // Non-public visibility is always anchored to a module inside the target that introduced
        // the binding. Cross-target access therefore needs a public re-export first.
        if importing_module.origin != binding.owner.origin {
            return Ok(false);
        }

        Ok(match &binding.visibility {
            VisibilityLevel::Private | VisibilityLevel::Self_ => {
                self.module_is_descendant_of(importing_module, binding.owner)?
            }
            VisibilityLevel::Crate => true,
            VisibilityLevel::Super => match self.env.parent_module(binding.owner)? {
                Some(visible_from) => {
                    self.module_is_descendant_of(importing_module, visible_from)?
                }
                None => false,
            },
            VisibilityLevel::Restricted(path) => {
                match self.restricted_visibility_owner(binding.owner, path)? {
                    Some(visible_from) => {
                        self.module_is_descendant_of(importing_module, visible_from)?
                    }
                    None => false,
                }
            }
            VisibilityLevel::Public => true,
            VisibilityLevel::Unknown(_) => false,
        })
    }

    /// Resolves the module that anchors a `pub(in path)` visibility restriction.
    fn restricted_visibility_owner(
        &self,
        owner: ModuleRef,
        path: &str,
    ) -> Result<Option<ModuleRef>, E::Error> {
        let mut segments = path.split("::");
        let Some(first) = segments.next() else {
            return Ok(None);
        };
        let mut current = match first {
            "crate" => {
                let Some(root) = self.env.root_module(owner.origin.origin_target())? else {
                    return Ok(None);
                };
                root
            }
            "self" => owner,
            "super" => {
                let Some(parent) = self.env.parent_module(owner)? else {
                    return Ok(None);
                };
                parent
            }
            _ => return Ok(None),
        };

        for segment in segments {
            let Some(module) = self.env.module_data(current)? else {
                return Ok(None);
            };
            let Some(child) = module
                .children
                .iter()
                .find_map(|(name, child)| (name == segment).then_some(*child))
            else {
                return Ok(None);
            };
            current = ModuleRef {
                origin: current.origin,
                module: child,
            };
        }

        Ok(Some(current))
    }
}
