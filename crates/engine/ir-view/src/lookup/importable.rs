//! Request-bounded discovery of definitions that can be reached by a `use` path.
//!
//! The frozen namespace graph already owns visibility, aliases, re-exports, and cfg filtering.
//! Walking that graph on demand keeps auto-import honest without retaining a second global symbol
//! index after the request finishes.

use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::Context as _;
use rg_def_map::{DefMapQuery, DefMapSource};
use rg_ir_model::{DefId, ModuleRef, Path, PathRoot, identity::DeclarationRef};
use rg_text::Name;

use crate::{
    IndexedViewDb, SymbolKind,
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

    pub fn path_len(&self) -> usize {
        self.path.component_count()
    }
}

/// Performs one prefix-aware graph walk and releases all traversal state with the request.
pub struct ImportableNameSearch<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> ImportableNameSearch<'a, 'db> {
    const MIN_PREFIX_LEN: usize = 2;
    const MAX_MODULES: usize = 512;
    const MAX_RESULTS: usize = 64;
    const MAX_DEPTH: usize = 6;

    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    fn path_rank(path: &Path) -> (usize, String) {
        (path.component_count(), path.to_string())
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
    /// result ceilings then bound both a cold stdlib walk and adversarial re-export graphs. If
    /// aliases expose one declaration several times, path length and spelling choose one stable
    /// representative.
    pub fn search(
        &self,
        importing_module: ModuleRef,
        prefix: &str,
    ) -> anyhow::Result<Vec<ImportableName>> {
        if prefix.chars().count() < Self::MIN_PREFIX_LEN {
            return Ok(Vec::new());
        }

        let mut pending = VecDeque::new();
        let importing_crate = importing_module.origin.origin_crate();
        let root_module = self
            .db
            .root_module(importing_crate)
            .context("read auto-import crate root")?;

        // A nested module can often name nearby definitions through a shorter `self` path. The
        // crate root remains a second seed so parent and sibling modules are still reachable.
        if Some(importing_module) != root_module {
            pending.push_back(PendingModule {
                module: importing_module,
                path: Path::new(PathRoot::SelfModule, Vec::new()),
                depth: 0,
            });
        }
        if let Some(root_module) = root_module {
            pending.push_back(PendingModule {
                module: root_module,
                path: Path::new(PathRoot::Crate, Vec::new()),
                depth: 0,
            });
        }

        let mut extern_roots = self
            .db
            .extern_roots(importing_crate)
            .context("read auto-import extern roots")?;
        extern_roots.sort_by(|(left, _), (right, _)| left.cmp(right));
        for (label, module) in extern_roots {
            pending.push_back(PendingModule {
                module,
                path: Path::relative(vec![Name::new(label)]),
                depth: 0,
            });
        }

        let def_maps = DefMapQuery::new(self.db);
        let names = NameLookupView::new(self.db);
        let mut visited = HashSet::new();
        let mut results: Vec<ImportableName> = Vec::new();
        let mut by_declaration = HashMap::new();

        while let Some(module) = pending.pop_front() {
            if visited.len() >= Self::MAX_MODULES || results.len() >= Self::MAX_RESULTS {
                break;
            }
            if !visited.insert(module.module) {
                continue;
            }

            let visible_defs = def_maps
                .visible_scope_defs(importing_module, module.module)
                .context("walk auto-import module scope")?;
            for visible_def in visible_defs {
                let is_module = matches!(visible_def.def, DefId::Module(_));
                if !is_module && !visible_def.label.starts_with(prefix) {
                    continue;
                }

                let Some(name) = names
                    .module_scope_name(importing_module, visible_def)
                    .context("read auto-import declaration")?
                else {
                    continue;
                };
                let path = {
                    let mut segments = module.path.segments().to_vec();
                    segments.push(Name::new(name.label()));
                    Path::new(module.path.root(), segments)
                };

                if name.kind() == SymbolKind::Module {
                    if module.depth < Self::MAX_DEPTH
                        && let DeclarationRef::Module(child) = name.declaration()
                    {
                        pending.push_back(PendingModule {
                            module: child,
                            path,
                            depth: module.depth + 1,
                        });
                    }
                    continue;
                }
                if name.namespace() == NameNamespace::Macros {
                    continue;
                }

                let key = (name.declaration(), name.namespace());
                if let Some(existing_index) = by_declaration.get(&key).copied() {
                    let existing: &mut ImportableName = &mut results[existing_index];
                    if Self::path_rank(&path) < Self::path_rank(existing.path()) {
                        *existing = ImportableName { name, path };
                    }
                    continue;
                }

                by_declaration.insert(key, results.len());
                results.push(ImportableName { name, path });
                if results.len() >= Self::MAX_RESULTS {
                    break;
                }
            }
        }

        results.sort_by(|left, right| {
            Self::path_rank(left.path())
                .cmp(&Self::path_rank(right.path()))
                .then(left.name().label().cmp(right.name().label()))
                .then(
                    format!("{:?}", left.name().declaration())
                        .cmp(&format!("{:?}", right.name().declaration())),
                )
        });
        Ok(results)
    }
}

#[derive(Debug)]
struct PendingModule {
    module: ModuleRef,
    path: Path,
    depth: usize,
}
