//! Helpers for resolving paths against def-map scopes.
//!
//! Resolution here is intentionally narrow:
//! - it works only with already-built module scopes
//! - it understands module navigation (`self`, `super`, `crate`)
//! - it can return multiple definitions because several namespaces may share one textual name
//!
//! During def-map construction this module reads from the fixed-point scope snapshot. After
//! construction, the same path-walking logic reads from frozen `DefMapDb` data.

use rg_ir_model::{DefId, DefMapRef, LocalDefRef, ModuleId, ModuleRef, TargetRef};
use rg_item_tree::VisibilityLevel;
use rg_package_store::PackageStoreError;
use rg_text::Name;

use crate::{
    DefMapReadTxn, ImportPath, LocalDefData, LocalDefKind, MacroDefinitionData, ModuleData, Path,
    PathSegment, ScopeBinding,
    model::{ModuleScopeBuilder, Namespace, ScopeEntryRef},
};

/// Result of resolving a path against the frozen def-map graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvePathResult {
    pub resolved: Vec<DefId>,
    pub unresolved_at: Option<usize>,
}

/// Minimal scope graph required by the path resolver.
pub(crate) trait PathResolutionEnv {
    fn extern_root(
        &self,
        target: TargetRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, PackageStoreError>;

    fn prelude_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError>;

    fn root_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError>;

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

impl PathResolutionEnv for DefMapReadTxn<'_> {
    fn extern_root(
        &self,
        target: TargetRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, PackageStoreError> {
        Ok(self
            .def_map(target)?
            .and_then(|def_map| def_map.extern_prelude().get(name).copied()))
    }

    fn prelude_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        Ok(self.def_map(target)?.and_then(|def_map| def_map.prelude()))
    }

    fn root_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        Ok(self.def_map(target)?.and_then(|def_map| {
            Some(ModuleRef {
                origin: DefMapRef::Target(target),
                module: def_map.root_module()?,
            })
        }))
    }

    fn module_data(&self, module_ref: ModuleRef) -> Result<Option<&ModuleData>, PackageStoreError> {
        let Some(target) = module_ref.origin.as_target_ref() else {
            return Ok(None);
        };
        Ok(self
            .def_map(target)?
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
        let Some(target) = local_def_ref.origin.as_target_ref() else {
            return Ok(None);
        };
        Ok(self
            .def_map(target)?
            .and_then(|def_map| def_map.local_def(local_def_ref.local_def)))
    }

    fn macro_definition_data(
        &self,
        local_def_ref: LocalDefRef,
    ) -> Result<Option<&MacroDefinitionData>, PackageStoreError> {
        let Some(target) = local_def_ref.origin.as_target_ref() else {
            return Ok(None);
        };
        Ok(self
            .def_map(target)?
            .and_then(|def_map| def_map.macro_definition(local_def_ref.local_def)))
    }
}

pub(crate) fn visible_module_scope_entry_set_with_env(
    env: &impl PathResolutionEnv,
    importing_module: ModuleRef,
    source_module: ModuleRef,
) -> Result<ModuleScopeBuilder, PackageStoreError> {
    let mut visible_scope = ModuleScopeBuilder::default();
    for (name, entry) in env.module_scope_entries(source_module)? {
        for binding in entry.types() {
            if binding_is_visible(env, importing_module, binding)? {
                visible_scope.insert_binding(name, Namespace::Types, binding.clone());
            }
        }

        for binding in entry.values() {
            if binding_is_visible(env, importing_module, binding)? {
                visible_scope.insert_binding(name, Namespace::Values, binding.clone());
            }
        }

        for binding in entry.macros() {
            if binding_is_visible(env, importing_module, binding)? {
                visible_scope.insert_binding(name, Namespace::Macros, binding.clone());
            }
        }
    }

    Ok(visible_scope)
}

/// Returns visible macro bindings for one name without copying the whole source scope.
pub(crate) fn visible_module_macro_bindings_with_env(
    env: &impl PathResolutionEnv,
    importing_module: ModuleRef,
    source_module: ModuleRef,
    name: &Name,
) -> Result<Vec<ScopeBinding>, PackageStoreError> {
    let Some(entry) = env.module_scope_entry(source_module, name.as_str())? else {
        return Ok(Vec::new());
    };

    let mut bindings = Vec::new();
    for binding in entry.macros() {
        if binding_is_visible(env, importing_module, binding)? {
            bindings.push(binding.clone());
        }
    }

    Ok(bindings)
}

/// Resolves a path to the definitions it denotes in the current scope snapshot.
///
/// The return type is a list rather than a single value because one textual name may resolve in
/// multiple namespaces at once.
pub(crate) fn resolve_path_to_defs_with_env(
    env: &impl PathResolutionEnv,
    importing_target: TargetRef,
    importing_module: ModuleId,
    path: &ImportPath,
) -> Result<Vec<DefId>, PackageStoreError> {
    resolve_path_to_defs_with_filter(
        env,
        importing_target,
        importing_module,
        path,
        NameResolutionFilter::AllNamespaces,
    )
}

fn resolve_path_to_defs_with_filter(
    env: &impl PathResolutionEnv,
    importing_target: TargetRef,
    importing_module: ModuleId,
    path: &ImportPath,
    terminal_filter: NameResolutionFilter,
) -> Result<Vec<DefId>, PackageStoreError> {
    let result = resolve_path_with_env(
        env,
        ModuleRef {
            origin: DefMapRef::Target(importing_target),
            module: importing_module,
        },
        path.absolute,
        &path.segments,
        terminal_filter,
    )?;

    Ok(result.resolved)
}

/// Resolves a path whose terminal segment must be a macro binding.
///
/// Macro expansion needs the binding origin of the final segment, because direct local
/// `macro_rules!` bindings are source-order sensitive while imports and `#[macro_export]` root
/// bindings are not.
pub(crate) fn resolve_path_to_macro_bindings_with_env(
    env: &impl PathResolutionEnv,
    importing_target: TargetRef,
    importing_module: ModuleId,
    path: &ImportPath,
) -> Result<Vec<ScopeBinding>, PackageStoreError> {
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
        resolve_path_to_modules_with_env(
            env,
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
        let Some(entry) = env.module_scope_entry(source_module, name.as_str())? else {
            continue;
        };
        for binding in entry.macros() {
            if binding_is_visible(env, importing_module_ref, binding)? {
                bindings.push(binding.clone());
            }
        }
    }

    Ok(bindings)
}

/// Resolves a path and keeps only module results.
///
/// This is used by glob imports, where the path must denote one or more source modules whose
/// contents will be copied into the importing scope.
pub(crate) fn resolve_path_to_modules_with_env(
    env: &impl PathResolutionEnv,
    importing_target: TargetRef,
    importing_module: ModuleId,
    path: &ImportPath,
) -> Result<Vec<ModuleRef>, PackageStoreError> {
    let resolved_defs = resolve_path_to_defs_with_filter(
        env,
        importing_target,
        importing_module,
        path,
        NameResolutionFilter::TypesOnly,
    )?;

    let mut modules = Vec::new();
    for resolved_def in resolved_defs {
        if let DefId::Module(module_ref) = resolved_def {
            if !modules.contains(&module_ref) {
                modules.push(module_ref);
            }
        }
    }

    Ok(modules)
}

/// Resolves a path against one read transaction.
pub(crate) fn resolve_path_in_txn(
    txn: &DefMapReadTxn<'_>,
    importing_module: ModuleRef,
    path: &Path,
) -> Result<ResolvePathResult, PackageStoreError> {
    resolve_path_with_env(
        txn,
        importing_module,
        path.absolute,
        &path.segments,
        NameResolutionFilter::AllNamespaces,
    )
}

/// Resolves a type-position path against one read transaction.
pub(crate) fn resolve_path_in_type_namespace_txn(
    txn: &DefMapReadTxn<'_>,
    importing_module: ModuleRef,
    path: &Path,
) -> Result<ResolvePathResult, PackageStoreError> {
    resolve_path_with_env(
        txn,
        importing_module,
        path.absolute,
        &path.segments,
        NameResolutionFilter::TypesOnly,
    )
}

pub(crate) fn namespace_for_def_with_env(
    env: &impl PathResolutionEnv,
    def: DefId,
) -> Result<Option<Namespace>, PackageStoreError> {
    match def {
        DefId::Module(_) => Ok(Some(Namespace::Types)),
        DefId::Local(local_def_ref) => Ok(env
            .local_def_kind(local_def_ref)?
            .map(|kind| kind.namespace())),
    }
}

/// Walks a path through one resolution environment.
fn resolve_path_with_env(
    env: &impl PathResolutionEnv,
    importing_module: ModuleRef,
    absolute: bool,
    segments: &[PathSegment],
    terminal_filter: NameResolutionFilter,
) -> Result<ResolvePathResult, PackageStoreError> {
    let Some((first_segment, remaining_segments)) = segments.split_first() else {
        return Ok(ResolvePathResult {
            resolved: Vec::new(),
            unresolved_at: Some(0),
        });
    };

    let mut current_defs = resolve_first_segment(
        env,
        importing_module,
        absolute,
        first_segment,
        NameResolutionFilter::for_segment(!remaining_segments.is_empty(), terminal_filter),
    )?;

    if current_defs.is_empty() {
        return Ok(ResolvePathResult {
            resolved: current_defs,
            unresolved_at: Some(0),
        });
    }

    for (segment_idx, segment) in remaining_segments.iter().enumerate() {
        current_defs = resolve_next_segment(
            env,
            importing_module,
            current_defs,
            segment,
            NameResolutionFilter::for_segment(
                segment_idx + 1 < remaining_segments.len(),
                terminal_filter,
            ),
        )?;

        if current_defs.is_empty() {
            return Ok(ResolvePathResult {
                resolved: current_defs,
                unresolved_at: Some(segment_idx + 1),
            });
        }
    }

    Ok(ResolvePathResult {
        resolved: current_defs,
        unresolved_at: None,
    })
}

/// Resolves the first path segment, which decides the starting search space.
///
/// Relative names first try the current module scope, then extern roots, then the standard
/// prelude. Absolute names skip local scope and prelude fallback entirely.
fn resolve_first_segment(
    env: &impl PathResolutionEnv,
    importing_module: ModuleRef,
    absolute: bool,
    segment: &PathSegment,
    filter: NameResolutionFilter,
) -> Result<Vec<DefId>, PackageStoreError> {
    if absolute {
        return match segment {
            PathSegment::Name(name) => Ok(env
                .extern_root(importing_module.origin.origin_target(), name)?
                .map(|module_ref| vec![DefId::Module(module_ref)])
                .unwrap_or_default()),
            PathSegment::SelfKw
            | PathSegment::SuperKw
            | PathSegment::CrateKw
            | PathSegment::DollarCrate(_) => Ok(Vec::new()),
        };
    }

    match segment {
        PathSegment::DollarCrate(target) => Ok(env
            .root_module(*target)?
            .map(DefId::Module)
            .into_iter()
            .collect()),
        PathSegment::SelfKw => Ok(vec![DefId::Module(importing_module)]),
        PathSegment::SuperKw => Ok(env
            .parent_module(importing_module)?
            .map(DefId::Module)
            .into_iter()
            .collect()),
        PathSegment::CrateKw => Ok(env
            .root_module(importing_module.origin.origin_target())?
            .map(DefId::Module)
            .into_iter()
            .collect()),
        PathSegment::Name(name) => {
            // Shadowing is namespace-specific. Prefixes and type-position terminals walk the
            // type namespace, so same-spelling value/macro bindings do not block fallback.
            let local_defs =
                resolve_name_in_module(env, importing_module, importing_module, name, filter)?;
            if !local_defs.is_empty() {
                return Ok(local_defs);
            }

            if let Some(module_ref) = env.extern_root(importing_module.origin.origin_target(), name)?
            {
                return Ok(vec![DefId::Module(module_ref)]);
            }

            let Some(prelude_module) =
                env.prelude_module(importing_module.origin.origin_target())?
            else {
                return Ok(Vec::new());
            };

            resolve_name_in_module(env, importing_module, prelude_module, name, filter)
        }
    }
}

/// Resolves every path segment after the first one.
///
/// At this point resolution can only continue through modules, so any non-module intermediate
/// definition is discarded.
fn resolve_next_segment(
    env: &impl PathResolutionEnv,
    importing_module: ModuleRef,
    current_defs: Vec<DefId>,
    segment: &PathSegment,
    filter: NameResolutionFilter,
) -> Result<Vec<DefId>, PackageStoreError> {
    let mut next_defs = Vec::new();

    for current_def in current_defs {
        let DefId::Module(module_ref) = current_def else {
            continue;
        };

        match segment {
            PathSegment::SelfKw => {
                push_unique_def(&mut next_defs, DefId::Module(module_ref));
            }
            PathSegment::SuperKw => {
                if let Some(parent) = env.parent_module(module_ref)? {
                    push_unique_def(&mut next_defs, DefId::Module(parent));
                }
            }
            PathSegment::CrateKw => {
                if let Some(root) = env.root_module(module_ref.origin.origin_target())? {
                    push_unique_def(&mut next_defs, DefId::Module(root));
                }
            }
            PathSegment::DollarCrate(_) => {}
            PathSegment::Name(name) => {
                for resolved_def in
                    resolve_name_in_module(env, importing_module, module_ref, name, filter)?
                {
                    push_unique_def(&mut next_defs, resolved_def);
                }
            }
        }
    }

    Ok(next_defs)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameResolutionFilter {
    AllNamespaces,
    TypesOnly,
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

/// Resolves one textual name inside one module scope.
///
/// The result is visibility-filtered from the perspective of the importing target, because
/// cross-target resolution is allowed to see only public bindings. The caller decides which
/// namespace buckets are meaningful for this path segment.
fn resolve_name_in_module(
    env: &impl PathResolutionEnv,
    importing_module: ModuleRef,
    module_ref: ModuleRef,
    name: &str,
    filter: NameResolutionFilter,
) -> Result<Vec<DefId>, PackageStoreError> {
    let Some(scope_entry) = env.module_scope_entry(module_ref, name)? else {
        return Ok(Vec::new());
    };

    let mut defs = Vec::new();

    // One textual name can contribute bindings from several namespaces, so we collect them all
    // into a deduplicated result set.
    for binding in scope_entry.types() {
        if binding_is_visible(env, importing_module, binding)? {
            push_unique_def(&mut defs, binding.def);
        }
    }

    if matches!(filter, NameResolutionFilter::TypesOnly) {
        return Ok(defs);
    }

    for binding in scope_entry.values() {
        if binding_is_visible(env, importing_module, binding)? {
            push_unique_def(&mut defs, binding.def);
        }
    }

    for binding in scope_entry.macros() {
        if binding_is_visible(env, importing_module, binding)? {
            push_unique_def(&mut defs, binding.def);
        }
    }

    Ok(defs)
}

/// Checks whether a binding can be observed from the importing module.
fn binding_is_visible(
    env: &impl PathResolutionEnv,
    importing_module: ModuleRef,
    binding: &ScopeBinding,
) -> Result<bool, PackageStoreError> {
    if matches!(binding.visibility, VisibilityLevel::Public) {
        return Ok(true);
    }

    // Non-public visibility is always anchored to a module inside the target that introduced the
    // binding. Cross-target access therefore needs a public re-export first.
    if importing_module.origin != binding.owner.origin {
        return Ok(false);
    }

    Ok(match &binding.visibility {
        VisibilityLevel::Private | VisibilityLevel::Self_ => {
            module_is_descendant_of(env, importing_module, binding.owner)?
        }
        VisibilityLevel::Crate => true,
        VisibilityLevel::Super => {
            match env.parent_module(binding.owner)? {
                Some(visible_from) => module_is_descendant_of(env, importing_module, visible_from)?,
                None => false,
            }
        }
        VisibilityLevel::Restricted(path) => {
            match restricted_visibility_owner(env, binding.owner, path)? {
                Some(visible_from) => module_is_descendant_of(env, importing_module, visible_from)?,
                None => false,
            }
        }
        VisibilityLevel::Public => true,
        VisibilityLevel::Unknown(_) => false,
    })
}

/// Resolves the module that anchors a `pub(in path)` visibility restriction.
fn restricted_visibility_owner(
    env: &impl PathResolutionEnv,
    owner: ModuleRef,
    path: &str,
) -> Result<Option<ModuleRef>, PackageStoreError> {
    let mut segments = path.split("::");
    let Some(first) = segments.next() else {
        return Ok(None);
    };
    let mut current = match first {
        "crate" => {
            let Some(root) = env.root_module(owner.origin.origin_target())? else {
                return Ok(None);
            };
            root
        }
        "self" => owner,
        "super" => {
            let Some(parent) = env.parent_module(owner)? else {
                return Ok(None);
            };
            parent
        }
        _ => return Ok(None),
    };

    for segment in segments {
        let Some(module) = env.module_data(current)? else {
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

/// Returns whether `module` is the same as or nested inside `ancestor`.
fn module_is_descendant_of(
    env: &impl PathResolutionEnv,
    module: ModuleRef,
    ancestor: ModuleRef,
) -> Result<bool, PackageStoreError> {
    if module.origin != ancestor.origin {
        return Ok(false);
    }

    let mut current = Some(module.module);
    while let Some(module_id) = current {
        if module_id == ancestor.module {
            return Ok(true);
        }

        current = env
            .module_data(ModuleRef {
                origin: module.origin,
                module: module_id,
            })?
            .and_then(|module| module.parent);
    }

    Ok(false)
}

/// Pushes one resolved definition unless it is already present in the result list.
fn push_unique_def(defs: &mut Vec<DefId>, def: DefId) {
    if !defs.contains(&def) {
        defs.push(def);
    }
}
