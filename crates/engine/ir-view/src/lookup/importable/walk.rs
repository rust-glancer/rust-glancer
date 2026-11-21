//! Walk the current module, crate, and dependencies in breadth-first order to find import paths.

use std::{
    borrow::Cow,
    collections::{HashSet, VecDeque},
    time::Instant,
};

use anyhow::Context as _;
use rg_def_map::{DefMapQuery, DefMapSource, DefMapView, ImportableScope, NameBinding};
use rg_ir_model::{DefId, ModuleRef, Path, PathRoot};
use rg_text::Name;

use crate::IndexedViewDb;

/// The breadth-first walk shared by spelling and definition lookup.
///
/// A visit exposes the module summary first. The caller can look for names and stop when
/// it has enough results, before enqueueing children through `descend`.
pub(crate) struct ImportableModuleWalk<'a, 'db> {
    pub(crate) db: &'a IndexedViewDb<'db>,
    pub(crate) importing_module: ModuleRef,
    deadline: Option<Instant>,
    pending: VecDeque<ModuleRoute>,
    scheduled: HashSet<ModuleRef>,
    unchecked_roots: HashSet<ModuleRef>,
}

impl<'a, 'db> ImportableModuleWalk<'a, 'db> {
    const MAX_MODULES: usize = 512;
    const MAX_DEPTH: usize = 6;

    pub(crate) fn new(
        db: &'a IndexedViewDb<'db>,
        importing_module: ModuleRef,
        deadline: Option<Instant>,
    ) -> anyhow::Result<Self> {
        let mut pending = VecDeque::new();
        let importing_crate = importing_module.origin.origin_crate();
        let root_module = db
            .root_module(importing_crate)
            .context("read auto-import crate root")?;

        // A nested module can often name nearby definitions through a shorter `self` path. The
        // crate root remains a second seed so parent and sibling modules are still reachable.
        if Some(importing_module) != root_module {
            pending.push_back(ModuleRoute {
                module: importing_module,
                path: Path::new(PathRoot::SelfModule, Vec::new()),
                depth: 0,
            });
        }
        if let Some(root_module) = root_module {
            pending.push_back(ModuleRoute {
                module: root_module,
                path: Path::new(PathRoot::Crate, Vec::new()),
                depth: 0,
            });
        }

        let mut extern_roots = db
            .extern_roots(importing_crate)
            .context("read auto-import extern roots")?;
        extern_roots.sort_by(|(left, _), (right, _)| left.cmp(right));
        let remaining = Self::MAX_MODULES - pending.len();
        let mut unchecked_roots = HashSet::new();
        for (label, module) in extern_roots.into_iter().take(remaining) {
            unchecked_roots.insert(module);
            pending.push_back(ModuleRoute {
                module,
                path: Path::relative(vec![Name::new(label)]),
                depth: 0,
            });
        }

        // Claim modules on enqueue so repeated re-exports cannot grow the queue.
        let scheduled = pending.iter().map(|route| route.module).collect();
        Ok(Self {
            db,
            importing_module,
            deadline,
            pending,
            scheduled,
            unchecked_roots,
        })
    }

    pub(crate) fn next(
        &mut self,
    ) -> anyhow::Result<Option<(ModuleRoute, DefMapView<'a>, Cow<'a, ImportableScope>)>> {
        while let Some(module) = self.pending.pop_front() {
            rg_std::check_cancel!(self.db, "importable module search");
            if self.deadline.is_some_and(|end| Instant::now() >= end) {
                break;
            }
            // Check seed visibility as the walk reaches it, after nearby modules.
            if module.depth == 0 {
                let unchecked = self.unchecked_roots.remove(&module.module);
                if !module_is_exposed(self.db, self.importing_module, module.module, None)? {
                    if unchecked {
                        self.scheduled.remove(&module.module);
                    }
                    continue;
                }
            }
            let Some(definitions) = self.db.definitions_for_origin(module.module.origin)? else {
                continue;
            };
            if let Some(summary) = definitions.importable_scope(module.module.module)? {
                return Ok(Some((module, definitions, summary)));
            }
        }
        Ok(None)
    }

    /// Use the small summary to follow visible module edges without loading their item scopes.
    pub(crate) fn descend(
        &mut self,
        module: &ModuleRoute,
        summary: &ImportableScope,
    ) -> anyhow::Result<()> {
        if module.depth >= Self::MAX_DEPTH || self.scheduled.len() >= Self::MAX_MODULES {
            return Ok(());
        }
        let def_maps = DefMapQuery::new(self.db);
        let resolver = def_maps.name_resolver();
        for (index, (label, binding)) in summary.modules().iter().enumerate() {
            if index % 64 == 0 {
                rg_std::check_cancel!(self.db, "importable module edges");
                if self.deadline.is_some_and(|end| Instant::now() >= end) {
                    break;
                }
            }
            let DefId::Module(child) = binding.def else {
                continue;
            };
            // A hidden root can also be reachable through a documented alias.
            // Validate the seeded spelling before treating its reservation as final.
            if self.scheduled.contains(&child)
                && (!self.unchecked_roots.contains(&child)
                    || module_is_exposed(self.db, self.importing_module, child, None)?)
            {
                continue;
            }
            if !resolver.binding_is_visible(self.importing_module, binding)?
                || !module_is_exposed(
                    self.db,
                    self.importing_module,
                    child,
                    Some((module.module, binding)),
                )?
            {
                continue;
            }
            self.unchecked_roots.remove(&child);
            self.scheduled.insert(child);
            self.pending.push_back(module.child(child, label));
            if self.scheduled.len() >= Self::MAX_MODULES {
                break;
            }
        }
        Ok(())
    }
}

/// One module waiting to be searched and the `use` path that reaches it from the request site.
#[derive(Debug)]
pub(crate) struct ModuleRoute {
    pub(crate) module: ModuleRef,
    path: Path,
    depth: usize,
}

impl ModuleRoute {
    pub(crate) fn path_with(&self, label: &Name) -> Path {
        let mut segments = self.path.segments().to_vec();
        segments.push(label.clone());
        Path::new(self.path.root(), segments)
    }

    fn child(&self, module: ModuleRef, label: &Name) -> Self {
        Self {
            module,
            path: self.path_with(label),
            depth: self.depth + 1,
        }
    }
}

/// A visible path can still pass through a hidden module in a dependency. A re-export
/// has its own attributes, so check the spelling that will appear in the import.
pub(crate) fn module_is_exposed(
    db: &IndexedViewDb<'_>,
    importing_module: ModuleRef,
    child: ModuleRef,
    route: Option<(ModuleRef, &NameBinding)>,
) -> anyhow::Result<bool> {
    let import = if let Some((source, binding)) = route
        && let Some(attrs) = binding.import_attrs()
    {
        if !binding.has_reexport() {
            return Ok(false);
        }
        Some((source.origin.origin_crate(), attrs))
    } else {
        None
    };
    let use_crate = importing_module.origin.origin_crate();
    let defining_crate = child.origin.origin_crate();
    if defining_crate == use_crate && import.is_none() {
        return Ok(true);
    }
    let attrs = db
        .module_data(child)?
        .map(|data| data.user_facing_attrs)
        .unwrap_or_default();
    Ok(attrs.is_suggestible_through(defining_crate, import, use_crate))
}
