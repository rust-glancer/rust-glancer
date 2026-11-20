//! Look up import paths by trait identity so methods can use re-exported traits.

use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    iter,
    time::Instant,
};

use rg_def_map::{NameBinding, Namespace, NamespaceSet};
use rg_ir_model::{DefId, LocalDefRef, ModuleRef, Path};
use rg_text::Name;

use super::{
    ImportableNameSearch,
    walk::{ImportableModuleWalk, ModuleRoute},
};
use crate::IndexedViewDb;

/// Find `use` paths for traits whose methods passed the receiver check.
///
/// A trait declared as `RenderExt` might be exposed as `pub use inner::RenderExt as Render;`.
/// Record type bindings by definition so a later batch of methods can find either spelling
/// in modules already visited. If a trait still has no path, resume the same walk. Check
/// visibility and resolve a path only when a batch asks for that definition.
pub(crate) struct DefinitionImportSearch<'a, 'db> {
    walk: ImportableModuleWalk<'a, 'db>,
    deadline: Instant,
    modules: Vec<ModuleRoute>,
    bindings: HashMap<LocalDefRef, DefinitionBindings<'a>>,
    paths: HashMap<LocalDefRef, Path>,
}

impl<'a, 'db> DefinitionImportSearch<'a, 'db> {
    pub(crate) fn new(
        db: &'a IndexedViewDb<'db>,
        importing_module: ModuleRef,
        deadline: Instant,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            walk: ImportableModuleWalk::new(db, importing_module, Some(deadline))?,
            deadline,
            modules: Vec::new(),
            bindings: HashMap::new(),
            paths: HashMap::new(),
        })
    }

    pub(crate) fn search(
        &mut self,
        definitions: &HashSet<LocalDefRef>,
    ) -> anyhow::Result<&HashMap<LocalDefRef, Path>> {
        // A new batch can use bindings seen anywhere in the walk. After that first lookup,
        // each iteration only needs to test the routes from the newly visited module.
        self.find_paths(definitions, 0)?;
        while definitions.iter().any(|def| !self.paths.contains_key(def)) {
            let Some((module, def_map, summary)) = self.walk.next()? else {
                break;
            };
            // Keep spellings for other definitions too. A later receiver check may ask
            // for a different trait that was re-exported here.
            let module_index = self.modules.len();
            if let Some(scope) = def_map.module_scope(module.module.module)? {
                for (index, (label, bindings)) in scope.entries().enumerate() {
                    if index % 64 == 0 {
                        rg_std::check_cancel!(self.walk.db, "importable definition bindings");
                        if Instant::now() >= self.deadline {
                            break;
                        }
                    }
                    let Some(binding) = bindings.binding(Namespace::Types) else {
                        continue;
                    };
                    let DefId::Local(def) = binding.def else {
                        continue;
                    };
                    if binding.attribute_import().is_some() && !binding.has_reexport() {
                        continue;
                    }
                    let route = DefinitionBinding {
                        module: module_index,
                        label,
                        binding,
                    };
                    match self.bindings.entry(def) {
                        Entry::Occupied(mut entry) => {
                            entry.get_mut().aliases.push(route);
                        }
                        Entry::Vacant(entry) => {
                            entry.insert(DefinitionBindings {
                                first: route,
                                aliases: Vec::new(),
                            });
                        }
                    }
                }
            }
            self.modules.push(module);
            self.find_paths(definitions, module_index)?;
            self.walk.descend(&self.modules[module_index], &summary)?;
        }
        Ok(&self.paths)
    }

    fn find_paths(
        &mut self,
        definitions: &HashSet<LocalDefRef>,
        first_module: usize,
    ) -> anyhow::Result<()> {
        let search = ImportableNameSearch::new(self.walk.db);
        for def in definitions {
            if self.paths.contains_key(def) {
                continue;
            }
            let Some(bindings) = self.bindings.get(def) else {
                continue;
            };
            let mut best = None;
            for route in iter::once(&bindings.first).chain(&bindings.aliases) {
                if route.module < first_module {
                    continue;
                }
                rg_std::check_cancel!(self.walk.db, "importable definition path");
                if Instant::now() >= self.deadline {
                    break;
                }
                if let Some(item) = search.importable_binding(
                    self.walk.importing_module,
                    &self.modules[route.module],
                    route.label,
                    Namespace::Types,
                    route.binding,
                    NamespaceSet::TYPES,
                )? {
                    let rank = item.path_rank();
                    if best.as_ref().is_none_or(|(best_rank, _)| rank < *best_rank) {
                        best = Some((rank, item.path));
                    }
                }
            }
            if let Some((_, path)) = best {
                self.paths.insert(*def, path);
            }
        }
        Ok(())
    }
}

struct DefinitionBindings<'a> {
    first: DefinitionBinding<'a>,
    aliases: Vec<DefinitionBinding<'a>>,
}

struct DefinitionBinding<'a> {
    module: usize,
    label: &'a Name,
    binding: &'a NameBinding,
}
