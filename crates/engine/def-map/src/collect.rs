//! Collects the unresolved def-map skeleton from item trees.
//!
//! This phase walks one target's module tree and records only what is directly visible from the
//! syntax:
//! - module hierarchy
//! - module-scope local definitions
//! - raw import directives
//! - immediate bindings such as child modules and `extern crate`
//!
//! Import resolution itself happens during build finalization, using the `base_scopes` produced
//! here.

use std::collections::HashMap;

use anyhow::Context as _;

use rg_item_tree::{
    Documentation, ExternCrateItem, ItemKind, ItemNode, ItemTreeDb, ItemTreeId, ItemTreeRef,
    ModuleItem, ModuleSource, Package as ItemTreePackage, UseImport, UseItem,
};
use rg_parse::{Package, Target};
use rg_text::Name;

use super::{
    DefId, DefMap, ImportBinding, ImportData, ImportKind, ImportPath, ImportSourcePath,
    LocalDefData, LocalDefKind, LocalDefRef, LocalImplData, ModuleData, ModuleId, ModuleOrigin,
    ModuleRef, ModuleScope, PackageSlot, ScopeBinding, TargetRef,
    scope::{ModuleScopeBuilder, Namespace},
};

/// Collected state for one target before fixed-point import resolution.
///
/// `def_map` contains the frozen structural data, while `base_scopes` keeps the directly known
/// bindings that later passes start from.
pub(super) struct TargetState {
    pub(super) target: TargetRef,
    pub(super) target_name: String,
    pub(super) def_map: DefMap,
    pub(super) base_scopes: Vec<ModuleScopeBuilder>,
    pub(super) implicit_roots: HashMap<Name, ModuleRef>,
    pub(super) prelude: Option<ModuleRef>,
}

/// Collects unresolved target states for every package/target pair.
///
/// The nested return shape mirrors the parsed package/target layout so later resolution can move
/// between targets by package slot and target slot.
pub(super) fn collect_target_states(
    packages: &[Package],
    item_tree: &ItemTreeDb,
    implicit_roots: &[Vec<HashMap<Name, ModuleRef>>],
) -> anyhow::Result<Vec<Vec<TargetState>>> {
    let mut states = Vec::with_capacity(packages.len());

    for (package_slot, package) in packages.iter().enumerate() {
        let item_tree_package = item_tree.package(package_slot).with_context(|| {
            format!(
                "while attempting to fetch item tree package for {}",
                package.package_name()
            )
        })?;
        states.push(collect_package_target_states(
            package_slot,
            package,
            item_tree_package,
            implicit_roots,
        )?);
    }

    Ok(states)
}

pub(super) fn collect_package_target_states(
    package_slot: usize,
    package: &Package,
    item_tree_package: &ItemTreePackage,
    implicit_roots: &[Vec<HashMap<Name, ModuleRef>>],
) -> anyhow::Result<Vec<TargetState>> {
    let mut package_states = Vec::with_capacity(package.targets().len());

    for target in package.targets() {
        let target_ref = TargetRef {
            package: PackageSlot(package_slot),
            target: target.id,
        };
        let target_roots = implicit_roots
            .get(package_slot)
            .and_then(|package_roots| package_roots.get(target.id.0))
            .expect("implicit roots should exist for every parsed target");
        let target_root = item_tree_package.target_root(target.id).with_context(|| {
            format!(
                "while attempting to fetch item tree target root for {}",
                target.name
            )
        })?;

        let collector = TargetScopeCollector::new(target_ref, target_roots);
        let state = collector
            .collect(item_tree_package, target, target_root.root_file)
            .with_context(|| {
                format!(
                    "while attempting to collect target scope for {}",
                    target.name
                )
            })?;
        package_states.push(state);
    }

    Ok(package_states)
}

/// Mutable collector for one target's module tree.
///
/// The collector builds two parallel structures:
/// - `def_map.modules`, which is the final structural payload
/// - `base_scopes`, which starts with only directly known bindings and is enriched later
struct TargetScopeCollector<'db> {
    target: TargetRef,
    implicit_roots: &'db HashMap<Name, ModuleRef>,
    def_map: DefMap,
    base_scopes: Vec<ModuleScopeBuilder>,
}

impl<'db> TargetScopeCollector<'db> {
    fn new(target: TargetRef, implicit_roots: &'db HashMap<Name, ModuleRef>) -> Self {
        Self {
            target,
            implicit_roots,
            def_map: DefMap::default(),
            base_scopes: Vec::new(),
        }
    }

    /// Walks the target starting from its root file and returns the unresolved target state.
    fn collect(
        mut self,
        item_tree: &ItemTreePackage,
        target: &Target,
        root_file: rg_parse::FileId,
    ) -> anyhow::Result<TargetState> {
        let root_file_tree = item_tree.file(root_file).with_context(|| {
            format!(
                "while attempting to fetch root item tree for {:?}",
                root_file
            )
        })?;
        // Root modules are identified by the target; they do not have a textual name or parent.
        let root_module = self.alloc_module(
            None,
            None,
            None,
            root_file_tree.docs.clone(),
            ModuleOrigin::Root {
                file_id: target.root_file,
            },
        );
        self.def_map.set_root_module(root_module);

        self.collect_items(item_tree, root_module, root_file, &root_file_tree.top_level)
            .context("while attempting to collect root file items")?;

        Ok(TargetState {
            target: self.target,
            target_name: target.name.clone(),
            def_map: self.def_map,
            base_scopes: self.base_scopes,
            implicit_roots: self.implicit_roots.clone(),
            prelude: None,
        })
    }

    /// Allocates one module in both the def-map payload and the base-scope table.
    fn alloc_module(
        &mut self,
        parent: Option<ModuleId>,
        name: Option<Name>,
        name_span: Option<rg_parse::Span>,
        docs: Option<rg_item_tree::Documentation>,
        origin: ModuleOrigin,
    ) -> ModuleId {
        let module_id = self.def_map.modules.alloc(ModuleData {
            name,
            name_span,
            docs,
            parent,
            children: Vec::new(),
            local_defs: Vec::new(),
            impls: Vec::new(),
            imports: Vec::new(),
            unresolved_imports: Vec::new(),
            scope: ModuleScope::default(),
            origin,
        });
        self.base_scopes.push(ModuleScopeBuilder::default());
        module_id
    }

    /// Walks one module's items and records everything that is immediately knowable.
    fn collect_items(
        &mut self,
        item_tree: &ItemTreePackage,
        module_id: ModuleId,
        file_id: rg_parse::FileId,
        items: &[ItemTreeId],
    ) -> anyhow::Result<()> {
        for item_id in items {
            let source = ItemTreeRef {
                file_id,
                item: *item_id,
            };
            let item = item_tree
                .item(source)
                .expect("item tree id should exist while collecting def map");
            match &item.kind {
                ItemKind::ExternCrate(extern_crate) => {
                    self.collect_extern_crate(module_id, item, extern_crate);
                }
                ItemKind::Module(module_item) => {
                    self.collect_module(item_tree, module_id, item, module_item)
                        .with_context(|| {
                            format!(
                                "while attempting to collect module {}",
                                item.name.as_deref().unwrap_or("<unnamed>")
                            )
                        })?;
                }
                ItemKind::Use(use_item) => {
                    self.collect_use(module_id, item, source, use_item);
                }
                ItemKind::Impl(_) => self.collect_local_impl(module_id, item, source),
                _ => self.collect_local_def(module_id, item, source),
            }
        }

        Ok(())
    }

    /// Records one module-scope local definition and inserts its direct binding into the base scope.
    fn collect_local_def(&mut self, module_id: ModuleId, item: &ItemNode, source: ItemTreeRef) {
        let Some(kind) = LocalDefKind::from_item_tag(item.kind.tag()) else {
            return;
        };
        let namespace = kind.namespace();
        let Some(name) = item.name.clone() else {
            return;
        };

        let local_def_id = self.def_map.local_defs.alloc(LocalDefData {
            module: module_id,
            name: name.clone(),
            kind,
            visibility: item.visibility.clone(),
            source,
            file_id: item.file_id,
            name_span: item.name_span,
            span: item.span,
        });
        self.def_map
            .modules
            .get_mut(module_id)
            .expect("module should exist for collected local definition")
            .local_defs
            .push(local_def_id);
        self.base_scopes
            .get_mut(module_id.0)
            .expect("base scope should exist for collected local definition")
            .insert_binding(
                &name,
                namespace,
                ScopeBinding {
                    def: DefId::Local(LocalDefRef {
                        target: self.target,
                        local_def: local_def_id,
                    }),
                    visibility: item.visibility.clone(),
                    owner: ModuleRef {
                        target: self.target,
                        module: module_id,
                    },
                },
            );
    }

    /// Records one module-scope impl block without inserting a namespace binding.
    fn collect_local_impl(&mut self, module_id: ModuleId, item: &ItemNode, source: ItemTreeRef) {
        let local_impl_id = self.def_map.local_impls.alloc(LocalImplData {
            module: module_id,
            source,
            file_id: item.file_id,
            span: item.span,
        });
        self.def_map
            .modules
            .get_mut(module_id)
            .expect("module should exist for collected impl block")
            .impls
            .push(local_impl_id);
    }

    /// Creates a child module node and recursively walks its item source when available.
    fn collect_module(
        &mut self,
        item_tree: &ItemTreePackage,
        parent_module: ModuleId,
        item: &ItemNode,
        module_item: &ModuleItem,
    ) -> anyhow::Result<()> {
        let Some(module_name) = item.name.clone() else {
            return Ok(());
        };

        let source = &module_item.source;
        let origin = match source {
            ModuleSource::Inline { .. } => ModuleOrigin::Inline {
                declaration_file: item.file_id,
                declaration_span: item.span,
            },
            ModuleSource::OutOfLine { definition_file } => ModuleOrigin::OutOfLine {
                declaration_file: item.file_id,
                declaration_span: item.span,
                definition_file: *definition_file,
            },
        };

        let inner_docs = match source {
            ModuleSource::Inline { .. } => module_item.inner_docs.clone(),
            ModuleSource::OutOfLine {
                definition_file: Some(definition_file),
            } => item_tree
                .file(*definition_file)
                .with_context(|| {
                    format!(
                        "while attempting to fetch out-of-line module docs for {:?}",
                        definition_file
                    )
                })?
                .docs
                .clone(),
            ModuleSource::OutOfLine {
                definition_file: None,
            } => None,
        };
        let child_module = self.alloc_module(
            Some(parent_module),
            Some(module_name.clone()),
            item.name_span,
            Documentation::concat(item.docs.clone(), inner_docs),
            origin,
        );
        self.link_child_module(
            parent_module,
            child_module,
            &module_name,
            item.visibility.clone(),
        );

        match source {
            ModuleSource::Inline { items } => {
                // Inline modules already carry their lowered items inside the parent file tree.
                self.collect_items(item_tree, child_module, item.file_id, items)
                    .context("while attempting to collect inline module items")?;
            }
            ModuleSource::OutOfLine {
                definition_file: Some(definition_file),
            } => {
                // Out-of-line modules point at another lowered file tree.
                let file_tree = item_tree.file(*definition_file).with_context(|| {
                    format!(
                        "while attempting to fetch out-of-line module item tree for {:?}",
                        definition_file
                    )
                })?;
                self.collect_items(
                    item_tree,
                    child_module,
                    *definition_file,
                    &file_tree.top_level,
                )
                .context("while attempting to collect out-of-line module items")?;
            }
            ModuleSource::OutOfLine {
                definition_file: None,
            } => {}
        }

        Ok(())
    }

    /// Links a child module into its parent's module tree and type namespace.
    fn link_child_module(
        &mut self,
        parent_module: ModuleId,
        child_module: ModuleId,
        module_name: &Name,
        visibility: rg_item_tree::VisibilityLevel,
    ) {
        self.def_map
            .modules
            .get_mut(parent_module)
            .expect("parent module should exist for child link")
            .children
            .push((module_name.clone(), child_module));
        self.base_scopes
            .get_mut(parent_module.0)
            .expect("base scope should exist for child link")
            .insert_binding(
                module_name,
                Namespace::Types,
                ScopeBinding {
                    def: DefId::Module(ModuleRef {
                        target: self.target,
                        module: child_module,
                    }),
                    visibility,
                    owner: ModuleRef {
                        target: self.target,
                        module: parent_module,
                    },
                },
            );
    }

    /// Records raw import directives for later fixed-point resolution.
    ///
    /// This phase only normalizes the path and binding metadata. It does not try to resolve the
    /// import yet.
    fn collect_use(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        source: ItemTreeRef,
        use_item: &UseItem,
    ) {
        let imports: &[UseImport] = &use_item.imports;

        for (import_index, import) in imports.iter().enumerate() {
            let path = ImportPath::from_use_path(&import.path);
            // Imports like `use foo::{self};` strip the trailing `self`. If nothing remains, there
            // is no path to record here.
            if path.segments.is_empty() {
                continue;
            }

            let import_id = self.def_map.imports.alloc(ImportData {
                module: module_id,
                visibility: item.visibility.clone(),
                kind: ImportKind::from_use_kind(import.kind),
                path,
                source_path: ImportSourcePath::from_use_path(&import.path),
                binding: ImportBinding::from_alias(&import.alias),
                alias_span: match &import.alias {
                    rg_item_tree::ImportAlias::Explicit { span, .. } => Some(*span),
                    rg_item_tree::ImportAlias::Inferred | rg_item_tree::ImportAlias::Hidden => None,
                },
                source,
                import_index,
            });
            self.def_map
                .modules
                .get_mut(module_id)
                .expect("module should exist for lowered import")
                .imports
                .push(import_id);
        }
    }

    /// Lowers `extern crate` into an immediate type-namespace binding.
    ///
    /// Unlike normal `use`, this can be bound during collection because the target roots are
    /// already known.
    fn collect_extern_crate(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        extern_crate: &ExternCrateItem,
    ) {
        let Some(extern_name) = extern_crate.name.clone() else {
            return;
        };
        let Some(binding_name) =
            ImportBinding::from_alias(&extern_crate.alias).resolve(Some(extern_name.clone()))
        else {
            return;
        };

        let module_ref = if extern_name == "self" {
            ModuleRef {
                target: self.target,
                module: self
                    .def_map
                    .root_module()
                    .expect("root module should exist before extern crate collection"),
            }
        } else {
            let Some(module_ref) = self.implicit_roots.get(&extern_name).copied() else {
                return;
            };
            module_ref
        };

        // `extern crate` contributes directly to the base scope rather than through a deferred
        // import record.
        self.base_scopes
            .get_mut(module_id.0)
            .expect("base scope should exist for extern crate binding")
            .insert_binding(
                &binding_name,
                Namespace::Types,
                ScopeBinding {
                    def: DefId::Module(module_ref),
                    visibility: item.visibility.clone(),
                    owner: ModuleRef {
                        target: self.target,
                        module: module_id,
                    },
                },
            );
    }
}
