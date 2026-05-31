// This lookup path is collected and tested before production body resolution is migrated to it.

use rg_def_map::{DefMap, ModuleOrigin, Path, PathSegment, ResolvePathResult, ScopeBinding};
use rg_ir_model::{DefId, ModuleRef};
use rg_item_tree::VisibilityLevel;

use super::push_unique;

/// Resolves paths against the body-local DefMap without pretending that body scopes are real modules.
// TODO: It resolves only direct body defmap scope bindings for now, e.g. no imports, absolute paths,
// prelude, or fixed-point behavior
pub(crate) struct BodyDefMapLookup<'body> {
    def_map: &'body DefMap,
}

impl<'body> BodyDefMapLookup<'body> {
    pub(crate) fn new(def_map: &'body DefMap) -> Self {
        Self { def_map }
    }

    pub(crate) fn resolve_path_in_type_namespace(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> ResolvePathResult {
        self.resolve_path_with_filter(from, path, NameResolutionFilter::TypesOnly)
    }

    pub(crate) fn resolve_path_in_value_namespace(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> ResolvePathResult {
        self.resolve_path_with_filter(from, path, NameResolutionFilter::ValuesOnly)
    }

    pub(crate) fn resolve_name_in_value_namespace_at_module(
        &self,
        from: ModuleRef,
        module: ModuleRef,
        name: &str,
    ) -> Vec<DefId> {
        self.resolve_name_in_module(from, module, name, NameResolutionFilter::ValuesOnly)
    }

    fn resolve_path_with_filter(
        &self,
        from: ModuleRef,
        path: &Path,
        terminal_filter: NameResolutionFilter,
    ) -> ResolvePathResult {
        if path.absolute || from.origin != self.def_map.own_ref() {
            return Self::unresolved_at(0);
        }

        let Some((first_segment, remaining_segments)) = path.segments.split_first() else {
            return Self::unresolved_at(0);
        };

        let mut current_defs = self.resolve_first_segment(
            from,
            first_segment,
            NameResolutionFilter::for_segment(!remaining_segments.is_empty(), terminal_filter),
        );
        if current_defs.is_empty() {
            return Self::unresolved_at(0);
        }

        for (idx, segment) in remaining_segments.iter().enumerate() {
            current_defs = self.resolve_next_segment(
                from,
                current_defs,
                segment,
                NameResolutionFilter::for_segment(
                    idx + 1 < remaining_segments.len(),
                    terminal_filter,
                ),
            );

            if current_defs.is_empty() {
                return Self::unresolved_at(idx + 1);
            }
        }

        ResolvePathResult {
            resolved: current_defs,
            unresolved_at: None,
        }
    }

    fn unresolved_at(segment: usize) -> ResolvePathResult {
        ResolvePathResult {
            resolved: Vec::new(),
            unresolved_at: Some(segment),
        }
    }

    fn resolve_first_segment(
        &self,
        from: ModuleRef,
        segment: &PathSegment,
        filter: NameResolutionFilter,
    ) -> Vec<DefId> {
        let PathSegment::Name(name) = segment else {
            return Vec::new();
        };

        // Body scopes are lexical. Starting from a synthetic module, an unqualified name walks
        // outward through synthetic parents until the first visible namespace hit.
        let mut current = Some(from.module);
        while let Some(module) = current {
            let module_ref = ModuleRef {
                origin: from.origin,
                module,
            };
            let defs = self.resolve_name_in_module(from, module_ref, name.as_str(), filter);
            if !defs.is_empty() {
                return defs;
            }

            let Some(module_data) = self.def_map.module(module) else {
                return Vec::new();
            };
            if !matches!(module_data.origin, ModuleOrigin::Synthetic { .. }) {
                break;
            }
            current = module_data.parent;
        }

        Vec::new()
    }

    fn resolve_next_segment(
        &self,
        from: ModuleRef,
        current_defs: Vec<DefId>,
        segment: &PathSegment,
        filter: NameResolutionFilter,
    ) -> Vec<DefId> {
        let PathSegment::Name(name) = segment else {
            return Vec::new();
        };

        let mut next_defs = Vec::new();
        for def in current_defs {
            let DefId::Module(module_ref) = def else {
                continue;
            };

            for resolved_def in self.resolve_name_in_module(from, module_ref, name.as_str(), filter)
            {
                push_unique(&mut next_defs, resolved_def);
            }
        }

        next_defs
    }

    fn resolve_name_in_module(
        &self,
        from: ModuleRef,
        module_ref: ModuleRef,
        name: &str,
        filter: NameResolutionFilter,
    ) -> Vec<DefId> {
        if module_ref.origin != self.def_map.own_ref() {
            return Vec::new();
        }

        let Some(scope_entry) = self
            .def_map
            .module(module_ref.module)
            .and_then(|module| module.scope.entry(name))
        else {
            return Vec::new();
        };

        let mut defs = Vec::new();
        if !matches!(filter, NameResolutionFilter::ValuesOnly) {
            for binding in scope_entry.types() {
                if self.binding_is_visible(from, binding) {
                    push_unique(&mut defs, binding.def);
                }
            }
        }

        if matches!(filter, NameResolutionFilter::TypesOnly) {
            return defs;
        }

        for binding in scope_entry.values() {
            if self.binding_is_visible(from, binding) {
                push_unique(&mut defs, binding.def);
            }
        }
        if matches!(filter, NameResolutionFilter::ValuesOnly) {
            return defs;
        }

        for binding in scope_entry.macros() {
            if self.binding_is_visible(from, binding) {
                push_unique(&mut defs, binding.def);
            }
        }

        defs
    }

    fn binding_is_visible(&self, from: ModuleRef, binding: &ScopeBinding) -> bool {
        if matches!(binding.visibility, VisibilityLevel::Public) {
            return true;
        }
        if from.origin != binding.owner.origin {
            return false;
        }

        match &binding.visibility {
            VisibilityLevel::Private | VisibilityLevel::Self_ => {
                self.module_is_descendant_of(from, binding.owner)
            }
            VisibilityLevel::Super => self
                .parent_module(binding.owner)
                .is_some_and(|parent| self.module_is_descendant_of(from, parent)),
            VisibilityLevel::Crate => true,
            VisibilityLevel::Public => true,
            VisibilityLevel::Restricted(_) | VisibilityLevel::Unknown(_) => false,
        }
    }

    fn parent_module(&self, module_ref: ModuleRef) -> Option<ModuleRef> {
        let parent = self.def_map.module(module_ref.module)?.parent?;
        Some(ModuleRef {
            origin: module_ref.origin,
            module: parent,
        })
    }

    fn module_is_descendant_of(&self, module: ModuleRef, ancestor: ModuleRef) -> bool {
        if module.origin != ancestor.origin {
            return false;
        }

        let mut current = Some(module.module);
        while let Some(module_id) = current {
            if module_id == ancestor.module {
                return true;
            }
            current = self
                .def_map
                .module(module_id)
                .and_then(|module| module.parent);
        }

        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameResolutionFilter {
    TypesOnly,
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
