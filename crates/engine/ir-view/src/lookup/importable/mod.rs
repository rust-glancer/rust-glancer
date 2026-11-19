//! Find a `use` path that makes a name available at the cursor.
//!
//! For `let _: HashM`, search the visible modules for matching names and keep the best
//! path found for each declaration. An import code action uses the same walk with the full name.

mod walk;

use std::{
    collections::{HashMap, hash_map::Entry},
    time::Instant,
};

use rg_def_map::{
    DefMapQuery, NameBinding, Namespace, NamespaceSet, VisibleScopeDef, VisibleScopeOrigin,
};
use rg_ir_model::{DefId, ModuleRef, Path, PathRoot};
use rg_text::Name;

use self::walk::{ImportableModuleWalk, ModuleRoute};
use crate::{
    IndexedViewDb,
    lookup::name::{ModuleScopeName, NameLookupView, NameNamespace},
};

/// A visible declaration paired with a path that can import it at the use site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportableName {
    name: ModuleScopeName,
    path: Path,
}

impl ImportableName {
    pub fn name(&self) -> &ModuleScopeName {
        &self.name
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Prefer fewer components, then break ties by the spelling of the import.
    fn path_rank(&self) -> (usize, String) {
        (self.path.component_count(), self.path.to_string())
    }
}

/// The declaration shape that may be imported at a completion site.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ImportPathKind {
    Type,
    TypeOrValue,
}

impl ImportPathKind {
    fn namespaces(self) -> NamespaceSet {
        match self {
            Self::Type => NamespaceSet::TYPES,
            Self::TypeOrValue => NamespaceSet::TYPES_VALUES,
        }
    }
}

/// Performs one bounded visible-name graph walk and releases all traversal state with the request.
///
/// Completion uses prefix matching, while explicit code actions use exact matching. Both entry
/// points share visibility, path ranking, re-export handling, and traversal ceilings here.
pub struct ImportableNameSearch<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> ImportableNameSearch<'a, 'db> {
    const MIN_PREFIX_LEN: usize = 2;
    const MAX_RESULTS: usize = 64;
    const MAX_COMPLETION_RESULTS: usize = 24;

    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub fn supports_prefix(prefix: &str) -> bool {
        prefix.chars().take(Self::MIN_PREFIX_LEN).count() == Self::MIN_PREFIX_LEN
    }

    /// Find importable type/value names whose exact spelling begins with `prefix`.
    ///
    /// ```text
    /// mod editor {
    ///     pub mod parser {
    ///         pub struct Parser;
    ///     }
    ///
    ///     fn load() {
    ///         let _: Par$0;
    ///     }
    /// }
    /// ```
    ///
    /// From `editor`, the same declaration is reachable as `self::parser::Parser` and
    /// `crate::editor::parser::Parser`. The result keeps the shorter path so source-edit planning
    /// does not need to understand the re-export graph.
    ///
    /// Two typed characters are required before global discovery starts. The module, depth, and
    /// result ceilings then bound both a cold stdlib walk and large re-export graphs. If the
    /// search reaches one declaration through several aliases, path length and spelling choose
    /// which of the paths seen so far to show.
    pub fn search(
        &self,
        importing_module: ModuleRef,
        prefix: &str,
        kind: ImportPathKind,
        deadline: Instant,
        occupied: impl Fn(&str, NameNamespace) -> bool,
    ) -> anyhow::Result<Vec<ImportableName>> {
        if !Self::supports_prefix(prefix) {
            return Ok(Vec::new());
        }

        self.search_matching(
            importing_module,
            Search::Prefix(prefix),
            kind.namespaces(),
            Some(deadline),
            Self::MAX_COMPLETION_RESULTS,
            occupied,
        )
    }

    /// Find importable type/value names whose complete visible spelling equals `name`.
    ///
    /// Explicit code actions already know the token the user wants to repair, so they do not need
    /// the completion prefix threshold. The same traversal ceilings still bound one-character
    /// names and large external graphs.
    pub fn search_exact(
        &self,
        importing_module: ModuleRef,
        name: &str,
    ) -> anyhow::Result<Vec<ImportableName>> {
        if name.is_empty() {
            return Ok(Vec::new());
        }

        self.search_matching(
            importing_module,
            Search::Exact(name),
            NamespaceSet::TYPES_VALUES,
            None,
            Self::MAX_RESULTS,
            |_, _| false,
        )
    }

    /// Walk paths visible from the importing module and collect leaves accepted by the search.
    ///
    /// For a nested importing module, the queue starts with `self` so nearby items get short paths.
    /// The crate root and dependency roots are also searched because they are valid beginnings of
    /// a `use` path. Entering a child module extends that path. Of the paths seen for a
    /// declaration, keep the shortest and break ties by spelling.
    fn search_matching(
        &self,
        importing_module: ModuleRef,
        search: Search<'_>,
        namespaces: NamespaceSet,
        deadline: Option<Instant>,
        max_results: usize,
        occupied: impl Fn(&str, NameNamespace) -> bool,
    ) -> anyhow::Result<Vec<ImportableName>> {
        let mut walk = ImportableModuleWalk::new(self.db, importing_module, deadline)?;
        let mut best_by_declaration = HashMap::new();

        let prefix = match search {
            Search::Prefix(prefix) | Search::Exact(prefix) => prefix,
        };
        'search: while let Some((module, definitions, summary)) = walk.next()? {
            // A prefix miss lets us skip this module's items. Its children can still
            // contain a match, as when only `std::collections` has a `HashMap` entry.
            if summary.may_contain(prefix)
                && let Some(scope) = definitions.module_scope(module.module.module)?
            {
                for (index, (label, bindings)) in scope.entries_with_prefix(prefix).enumerate() {
                    if index % 64 == 0 {
                        rg_std::check_cancel!(self.db, "importable name search");
                        if deadline.is_some_and(|end| Instant::now() >= end) {
                            break 'search;
                        }
                    }
                    if matches!(search, Search::Exact(name) if label.as_str() != name) {
                        break;
                    }
                    for namespace in namespaces.iter() {
                        let Some(binding) = bindings.binding(namespace) else {
                            continue;
                        };
                        if occupied(label.as_str(), namespace.into()) {
                            continue;
                        }
                        let Some(candidate) = self.importable_binding(
                            importing_module,
                            &module,
                            label,
                            namespace,
                            binding,
                            namespaces,
                        )?
                        else {
                            continue;
                        };

                        // A re-export gives the same declaration another path. Only a new
                        // declaration in this namespace needs another place in the result.
                        let key = (candidate.name.declaration(), candidate.name.namespace());
                        let first_seen = best_by_declaration.len();
                        match best_by_declaration.entry(key) {
                            Entry::Vacant(slot) => {
                                slot.insert((first_seen, candidate));
                                if best_by_declaration.len() >= max_results {
                                    break 'search;
                                }
                            }
                            Entry::Occupied(mut slot) => {
                                let (_, best) = slot.get_mut();
                                if candidate.path_rank() < best.path_rank() {
                                    *best = candidate;
                                }
                            }
                        }
                    }
                }
            }

            walk.descend(&module, &summary)?;
        }

        // A unit struct can have the same path and declaration in two namespaces.
        // Preserve discovery order for tied rows, even if we later found a shorter path.
        let mut results: Vec<_> = best_by_declaration.into_values().collect();
        results.sort_by_cached_key(|(first_seen, item)| {
            (
                item.path_rank(),
                format!("{:?}", item.name().declaration()),
                *first_seen,
            )
        });
        Ok(results.into_iter().map(|(_, item)| item).collect())
    }

    /// Turn a scope binding into a path the importing module can actually use. In particular,
    /// a name visible inside the walked module can still be hidden from the caller.
    fn importable_binding(
        &self,
        importing_module: ModuleRef,
        module: &ModuleRoute,
        label: &Name,
        namespace: Namespace,
        binding: &NameBinding,
        namespaces: NamespaceSet,
    ) -> anyhow::Result<Option<ImportableName>> {
        let def_maps = DefMapQuery::new(self.db);
        let resolver = def_maps.name_resolver();
        if !resolver.binding_is_visible(importing_module, binding)? {
            return Ok(None);
        }
        let Some(name) = self.binding_name(importing_module, label, namespace, binding)? else {
            return Ok(None);
        };
        let mut path = module.path_with(label);
        if !resolver
            .resolve_path(importing_module, &path, namespaces)?
            .resolved
            .contains(&binding.def)
        {
            if path.root() != PathRoot::Relative {
                return Ok(None);
            }
            // An extern crate can be shadowed by a module in the importing crate.
            path = Path::absolute(path.segments().to_vec());
            if !resolver
                .resolve_path(importing_module, &path, namespaces)?
                .resolved
                .contains(&binding.def)
            {
                return Ok(None);
            }
        }
        Ok(Some(ImportableName { name, path }))
    }

    fn binding_name(
        &self,
        importing_module: ModuleRef,
        label: &Name,
        namespace: Namespace,
        binding: &NameBinding,
    ) -> anyhow::Result<Option<ModuleScopeName>> {
        if matches!(binding.def, DefId::Module(_))
            || (binding.attribute_import().is_some() && !binding.has_reexport())
        {
            return Ok(None);
        }

        NameLookupView::new(self.db).module_scope_name(
            importing_module,
            VisibleScopeDef {
                label: label.to_string(),
                namespace,
                def: binding.def,
                origin: VisibleScopeOrigin::ItemScope,
                attribute_import: binding.attribute_import(),
                has_reexport: binding.has_reexport(),
            },
        )
    }
}

#[derive(Clone, Copy)]
enum Search<'a> {
    Prefix(&'a str),
    Exact(&'a str),
}
