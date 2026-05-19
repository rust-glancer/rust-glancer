//! Collects source produced by macro expansion back into mutable target state.
//!
//! Expansion is parsed as ordinary Rust source for this milestone. Generated definitions belong to
//! the macro call's module and file identity, while their spans point at the call site.

use anyhow::Result;

use rg_item_tree::{
    CfgExpr, Documentation, ImportAlias, ItemTreeRef, MacroCallItem, MacroDefinitionItem, UseItem,
    VisibilityLevel,
};
use rg_parse::{FileId, Span};
use rg_syntax::{
    SourceFile,
    ast::{self, HasModuleItem, HasName, HasVisibility},
};
use rg_text::NameInterner;

use crate::{
    DefId, ImportBinding, ImportData, ImportKind, ImportPath, ImportSourcePath, LocalDefData,
    LocalDefKind, LocalDefRef, MacroDefinitionData, ModuleData, ModuleId, ModuleOrigin, ModuleRef,
    ModuleScope, ScopeBinding,
    build::{
        cfg::CfgEvaluator, collect::TargetState, finalize::ScopeMatrix,
        stats::DefMapFinalizationStatsSink,
    },
    model::Namespace,
};

use super::{
    ItemOrder, MacroCallSite, MacroDefinitionRecord, MacroExpansionApplyResult, macro_edition,
};

/// Call-site identity used for every item produced by one macro expansion.
#[derive(Debug, Clone)]
pub(super) struct GeneratedOrigin {
    pub(super) module: ModuleId,
    pub(super) source: ItemTreeRef,
    pub(super) file_id: FileId,
    pub(super) span: Span,
    pub(super) order: ItemOrder,
}

/// Small collector that mirrors normal def-map collection for already-expanded source.
pub(super) struct GeneratedCollector<'a> {
    pub(super) state: &'a mut TargetState,
    pub(super) interner: &'a mut NameInterner,
    pub(super) current_scopes: &'a mut ScopeMatrix,
    pub(super) origin: GeneratedOrigin,
    pub(super) result: MacroExpansionApplyResult,
}

impl GeneratedCollector<'_> {
    pub(super) fn collect_source(
        &mut self,
        source: &str,
        macro_name: Option<&str>,
        stats: &mut DefMapFinalizationStatsSink<'_>,
    ) -> Result<MacroExpansionApplyResult> {
        // Parse generated text as a source file, then collect only item-position declarations from
        // the resulting syntax tree.
        let timer = stats.start_timer();
        let parsed = SourceFile::parse(source, macro_edition(self.state.edition))
            .ok()
            .map_err(|errors| {
                anyhow::anyhow!("macro expansion source has syntax errors: {errors:?}")
            });
        stats.finish_timer(timer, |timings, elapsed| {
            timings.parse_generated_sources += elapsed;
        });
        let file = match parsed {
            Ok(file) => file,
            Err(error) => {
                let macro_name = macro_name.unwrap_or("<unknown>");
                stats.record(|stats| stats.record_generated_source_parse_failure(macro_name));
                return Err(error);
            }
        };
        stats.record(|stats| stats.generated_sources_parsed += 1);
        self.result.mark_changed();

        let items = file.items().collect::<Vec<_>>();

        // Generated items may introduce further macro calls, imports, and inline modules. Those are
        // appended to the same mutable state so the surrounding fixed-point loop can see them.
        let timer = stats.start_timer();
        for (index, item) in items.into_iter().enumerate() {
            self.collect_item(
                self.origin.module,
                item,
                self.origin.order.generated_child(index),
                stats,
            );
        }
        stats.finish_timer(timer, |timings, elapsed| {
            timings.collect_generated_items += elapsed;
        });

        Ok(self.result)
    }

    fn collect_item(
        &mut self,
        module_id: ModuleId,
        item: ast::Item,
        order: ItemOrder,
        stats: &mut DefMapFinalizationStatsSink<'_>,
    ) {
        if !self.is_item_enabled(&item) {
            return;
        }
        stats.record(|stats| stats.generated_items_seen += 1);

        match item {
            ast::Item::Const(item) => {
                self.collect_named_def(module_id, &item, LocalDefKind::Const, None);
            }
            ast::Item::Enum(item) => {
                self.collect_named_def(module_id, &item, LocalDefKind::Enum, None);
            }
            ast::Item::Fn(item) => {
                self.collect_named_def(module_id, &item, LocalDefKind::Function, None);
            }
            ast::Item::MacroCall(item) => {
                let macro_call = MacroCallItem::from_ast(&item, self.interner);
                self.state.push_macro_call(MacroCallSite {
                    module: module_id,
                    source: self.origin.source,
                    path: macro_call.path,
                    callee: macro_call.callee,
                    args: macro_call.args,
                    file_id: self.origin.file_id,
                    span: self.origin.span,
                    order,
                });
            }
            ast::Item::MacroDef(item) => {
                let macro_definition = MacroDefinitionItem::from_macro_def(&item);
                self.collect_named_def(
                    module_id,
                    &item,
                    LocalDefKind::MacroDefinition,
                    Some((macro_definition, order)),
                );
            }
            ast::Item::MacroRules(item) => {
                let macro_definition = MacroDefinitionItem::from_macro_rules(&item);
                self.collect_named_def(
                    module_id,
                    &item,
                    LocalDefKind::MacroDefinition,
                    Some((macro_definition, order)),
                );
            }
            ast::Item::Module(item) => self.collect_inline_module(module_id, &item, order, stats),
            ast::Item::Static(item) => {
                self.collect_named_def(module_id, &item, LocalDefKind::Static, None);
            }
            ast::Item::Struct(item) => {
                self.collect_named_def(module_id, &item, LocalDefKind::Struct, None);
            }
            ast::Item::Trait(item) => {
                self.collect_named_def(module_id, &item, LocalDefKind::Trait, None);
            }
            ast::Item::TypeAlias(item) => {
                self.collect_named_def(module_id, &item, LocalDefKind::TypeAlias, None);
            }
            ast::Item::Union(item) => {
                self.collect_named_def(module_id, &item, LocalDefKind::Union, None);
            }
            ast::Item::Use(item) => self.collect_use(module_id, &item),
            ast::Item::AsmExpr(_)
            | ast::Item::ExternBlock(_)
            | ast::Item::ExternCrate(_)
            | ast::Item::Impl(_) => {}
        }
    }

    fn is_item_enabled(&self, item: &ast::Item) -> bool {
        let cfg = CfgExpr::from_attrs(item);
        CfgEvaluator::new(&self.state.cfg_options, &self.state.target_kind).is_enabled(&cfg)
    }

    fn collect_named_def<T>(
        &mut self,
        module_id: ModuleId,
        item: &T,
        kind: LocalDefKind,
        macro_definition: Option<(MacroDefinitionItem, ItemOrder)>,
    ) where
        T: AstNodeWithNameAndVisibility,
    {
        let Some(name) = item.name().map(|name| self.interner.intern(name.text())) else {
            return;
        };
        let visibility = VisibilityLevel::from_ast(item.visibility());
        let local_def_id = self.state.def_map.alloc_local_def(LocalDefData {
            module: module_id,
            name: name.clone(),
            kind,
            visibility: visibility.clone(),
            source: self.origin.source,
            file_id: self.origin.file_id,
            name_span: Some(self.origin.span),
            span: self.origin.span,
        });
        self.state
            .def_map
            .module_mut(module_id)
            .expect("module should exist for generated local definition")
            .local_defs
            .push(local_def_id);
        let binding = ScopeBinding {
            def: DefId::Local(LocalDefRef {
                target: self.state.target,
                local_def: local_def_id,
            }),
            visibility,
            owner: ModuleRef {
                target: self.state.target,
                module: module_id,
            },
        };
        // Update both the base scopes and the current snapshot. The base scopes make future import
        // refreshes see the generated name; the current snapshot lets later generated calls in this
        // pass resolve it immediately.
        self.state
            .base_scopes
            .get_mut(module_id.0)
            .expect("base scope should exist for generated local definition")
            .insert_binding(&name, kind.namespace(), binding.clone());
        self.current_scopes
            .module_scope_mut(self.state.target, module_id)
            .expect("current scope should exist for generated local definition")
            .insert_binding(&name, kind.namespace(), binding);

        if let Some((item, order)) = macro_definition {
            self.state
                .macro_definitions
                .insert(local_def_id, MacroDefinitionRecord { order });
            self.state.def_map.insert_macro_definition(
                local_def_id,
                MacroDefinitionData::from_item(&item, self.state.edition),
            );
        }
    }

    fn collect_inline_module(
        &mut self,
        parent_module: ModuleId,
        item: &ast::Module,
        order: ItemOrder,
        stats: &mut DefMapFinalizationStatsSink<'_>,
    ) {
        let Some(module_name) = item.name().map(|name| self.interner.intern(name.text())) else {
            return;
        };
        let Some(item_list) = item.item_list() else {
            // Out-of-line generated modules need call-site module file resolution. Skipping them is
            // a false negative, while inventing an empty module would create misleading scope data.
            return;
        };

        let visibility = VisibilityLevel::from_ast(item.visibility());
        let child_module = self.state.def_map.alloc_module(ModuleData {
            name: Some(module_name.clone()),
            name_span: Some(self.origin.span),
            docs: Documentation::inner_from_ast(item),
            parent: Some(parent_module),
            children: Vec::new(),
            local_defs: Vec::new(),
            impls: Vec::new(),
            imports: Vec::new(),
            unresolved_imports: Vec::new(),
            scope: ModuleScope::default(),
            origin: ModuleOrigin::Inline {
                declaration_file: self.origin.file_id,
                declaration_span: self.origin.span,
            },
        });
        // Inline generated modules extend all scope matrices in lockstep with the def-map module
        // arena so later generated children can be collected into the new module.
        self.state.base_scopes.push(Default::default());
        self.current_scopes
            .push_module_scope(self.state.target, Default::default())
            .expect("current scopes should have a target slot for generated module");
        self.state
            .def_map
            .module_mut(parent_module)
            .expect("parent module should exist for generated child link")
            .children
            .push((module_name.clone(), child_module));
        let binding = ScopeBinding {
            def: DefId::Module(ModuleRef {
                target: self.state.target,
                module: child_module,
            }),
            visibility,
            owner: ModuleRef {
                target: self.state.target,
                module: parent_module,
            },
        };
        self.state
            .base_scopes
            .get_mut(parent_module.0)
            .expect("base scope should exist for generated child link")
            .insert_binding(&module_name, Namespace::Types, binding.clone());
        self.current_scopes
            .module_scope_mut(self.state.target, parent_module)
            .expect("current scope should exist for generated child link")
            .insert_binding(&module_name, Namespace::Types, binding);

        for (index, child_item) in item_list.items().enumerate() {
            self.collect_item(
                child_module,
                child_item,
                order.generated_child(index),
                stats,
            );
        }
    }

    fn collect_use(&mut self, module_id: ModuleId, item: &ast::Use) {
        let visibility = VisibilityLevel::from_ast(item.visibility());
        let use_item = UseItem::from_ast(item, self.interner);

        for (import_index, import) in use_item.imports.iter().enumerate() {
            let path = ImportPath::from_use_path(&import.path);
            if path.segments.is_empty() {
                continue;
            }

            // The generated import's textual source is synthetic. Keep spans at the macro call site
            // so diagnostics and navigation have a real file location to point at.
            let mut source_path = ImportSourcePath::from_use_path(&import.path);
            source_path.source_span = Some(self.origin.span);
            for segment in &mut source_path.segments {
                segment.span = self.origin.span;
            }

            let import_id = self.state.def_map.alloc_import(ImportData {
                module: module_id,
                visibility: visibility.clone(),
                kind: ImportKind::from_use_kind(import.kind),
                path,
                source_path,
                binding: ImportBinding::from_alias(&import.alias),
                alias_span: match &import.alias {
                    ImportAlias::Explicit { .. } => Some(self.origin.span),
                    ImportAlias::Inferred | ImportAlias::Hidden => None,
                },
                source: self.origin.source,
                import_index,
            });
            self.state
                .def_map
                .module_mut(module_id)
                .expect("module should exist for generated import")
                .imports
                .push(import_id);
        }
    }
}

trait AstNodeWithNameAndVisibility: ast::HasName + ast::HasVisibility {}

impl<T> AstNodeWithNameAndVisibility for T where T: ast::HasName + ast::HasVisibility {}
