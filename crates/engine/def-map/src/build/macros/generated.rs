//! Collects syntax produced by macro expansion back into mutable crate state.
//!
//! Generated definitions belong to the macro call's module and file identity. Their retained item
//! payloads carry expansion spans where available, while generated imports and other provenance-only
//! facts may still point at the macro call site.

use anyhow::{Context as _, Result};

use crate::{
    ImportBinding, ImportData, ImportKind, ImportPath, LocalDefData, LocalDefKind, LocalImplData,
    MacroDefinitionData, ModuleData, ModuleOrigin, ModuleScope, Namespace, ScopeBinding,
    ScopeBindingProvenance, Visibility,
};
use rg_ir_model::{CrateRef, DefId, DefMapRef, LocalDefId, LocalDefRef, ModuleId, ModuleRef};
use rg_item_tree::{
    Documentation, ExternBlockItem, ImportAlias, ItemKind, ItemNode, ItemTreeId, ItemTreeRef,
    MacroCallItem, MacroDefinitionAttrs, MacroDefinitionItem, ModuleItem, ModuleSource, UseImport,
    UseItem,
};
use rg_macro_runtime::ExpansionSyntax;
use rg_parse::{FileId, Span};
use rg_text::{Name, NameInterner};

use crate::build::{collect::CrateState, finalize::ScopeMatrix};
use crate::profile::metric;
use crate::{GeneratedItemRef, GeneratedSourceId, ItemSource};

use super::{
    ItemOrder, MacroCallOrigin, MacroCallSite, MacroDefinitionRecord, MacroExpansionApplyResult,
    generated_tree::GeneratedSourceLowering,
};

/// Call-site identity used for every item produced by one macro expansion.
#[derive(Debug, Clone)]
pub(super) struct GeneratedOrigin {
    pub(super) module: ModuleId,
    pub(super) source: ItemTreeRef,
    pub(super) file_id: FileId,
    pub(super) span: Span,
    pub(super) order: ItemOrder,
    pub(super) dollar_crate: Option<CrateRef>,
    pub(super) parent_call: usize,
}

/// Small collector that mirrors normal def-map collection for already-expanded syntax.
pub(super) struct GeneratedCollector<'a> {
    pub(super) state: &'a mut CrateState,
    pub(super) interner: &'a mut NameInterner,
    pub(super) current_scopes: &'a mut ScopeMatrix,
    pub(super) origin: GeneratedOrigin,
    pub(super) result: MacroExpansionApplyResult,
}

impl GeneratedCollector<'_> {
    pub(super) fn collect_syntax(
        &mut self,
        expansion: ExpansionSyntax,
        macro_name: Option<&str>,
    ) -> Result<MacroExpansionApplyResult> {
        // Macro expansion has already run the parser over token trees. At this point we only check
        // syntax errors and collect item-position declarations from the generated root.
        let timer = metric::TIMING_PARSE_GENERATED_SOURCES.start_timer();
        let errors = expansion.parse.errors();
        timer.finish();
        if !errors.is_empty() {
            let macro_name = macro_name.unwrap_or("<unknown>");
            metric::MACRO_CALLS_FAILED.inc();
            metric::GENERATED_SOURCE_PARSE_FAILURES.inc();
            metric::FAILED_PARSE_BY_NAME.inc(macro_name);
            anyhow::bail!("macro expansion syntax has errors: {errors:?}");
        }
        metric::GENERATED_SOURCES_PARSED.inc();
        self.result.mark_changed();

        let generated_source = GeneratedSourceLowering::lower(
            &self.origin,
            expansion,
            self.interner,
            self.state.edition,
        )
        .context("while attempting to lower macro expansion into generated source")?;
        let generated_source_id = self
            .state
            .def_map_builder
            .alloc_generated_source(generated_source);
        let top_level = self
            .state
            .def_map_builder
            .partial()
            .generated_source(generated_source_id)
            .expect("generated source should exist immediately after allocation")
            .top_level
            .clone();

        // Generated items may introduce further macro calls, imports, and inline modules. Those are
        // appended to the same mutable state so the surrounding fixed-point loop can see them.
        let timer = metric::TIMING_COLLECT_GENERATED_ITEMS.start_timer();
        for (index, item_id) in top_level.into_iter().enumerate() {
            self.collect_item(
                self.origin.module,
                generated_source_id,
                item_id,
                self.origin.order.generated_child(index),
            );
        }
        timer.finish();

        Ok(self.result)
    }

    fn collect_item(
        &mut self,
        module_id: ModuleId,
        generated_source: GeneratedSourceId,
        item_id: ItemTreeId,
        order: ItemOrder,
    ) {
        let item = self
            .state
            .def_map_builder
            .partial()
            .generated_source(generated_source)
            .and_then(|source| source.item(item_id))
            .expect("generated item id should exist while collecting def map")
            .clone();
        if !self.is_item_enabled(&item) {
            return;
        }
        metric::GENERATED_ITEMS_SEEN.inc();

        match &item.kind {
            ItemKind::MacroCall(macro_call) => {
                self.collect_macro_call(module_id, &item, macro_call, order);
            }
            ItemKind::MacroDefinition(macro_definition) => {
                self.collect_macro_definition(
                    module_id,
                    &item,
                    macro_definition,
                    generated_source,
                    item_id,
                    order,
                );
            }
            ItemKind::Module(module_item) => {
                self.collect_inline_module(module_id, &item, module_item, generated_source, order);
            }
            ItemKind::Use(use_item) => self.collect_use(module_id, &item, use_item),
            ItemKind::Enum(enum_item) => {
                self.collect_enum(module_id, &item, enum_item, generated_source, item_id)
            }
            ItemKind::Impl(_) => {
                self.collect_local_impl(module_id, &item, generated_source, item_id)
            }
            ItemKind::ExternBlock(extern_block) => {
                self.collect_extern_block(module_id, extern_block, generated_source, item_id)
            }
            ItemKind::AsmExpr | ItemKind::ExternCrate(_) => {}
            _ => {
                self.collect_named_def(
                    module_id,
                    &item,
                    generated_source,
                    item_id,
                    ScopeBindingProvenance::Direct,
                );
            }
        }
    }

    /// Collects supported children of a generated extern block into the call-site module.
    fn collect_extern_block(
        &mut self,
        module_id: ModuleId,
        extern_block: &ExternBlockItem,
        generated_source: GeneratedSourceId,
        block_id: ItemTreeId,
    ) {
        let block_source = self.item_source(generated_source, block_id);
        for child_id in &extern_block.items {
            let child = self
                .state
                .def_map_builder
                .partial()
                .generated_source(generated_source)
                .and_then(|source| source.item(*child_id))
                .expect("generated extern-block child should exist while collecting def map")
                .clone();
            if !self.is_item_enabled(&child) {
                continue;
            }
            // Retained nested macro calls are intentionally not added to the expansion queue. The
            // ordinary local-def classifier rejects them here.
            if let Some(local_def) = self.collect_named_def(
                module_id,
                &child,
                generated_source,
                *child_id,
                ScopeBindingProvenance::Direct,
            ) {
                self.state
                    .def_map_builder
                    .insert_foreign_block(local_def, block_source);
            }
        }
    }

    fn is_item_enabled(&self, item: &ItemNode) -> bool {
        self.state.cfg_evaluator().is_enabled(&item.cfg)
    }

    fn collect_named_def(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        generated_source: GeneratedSourceId,
        item_id: ItemTreeId,
        provenance: ScopeBindingProvenance,
    ) -> Option<LocalDefId> {
        let kind = LocalDefKind::from_item_tag(item.kind.tag())?;
        let namespaces = kind.scope_namespaces(&item.kind);
        let name = item.name.clone()?;
        let visibility = item.visibility.clone();
        let visibilities = self.state.def_map_builder.resolve_local_def_visibilities(
            module_id,
            &item.kind,
            &visibility,
        );
        let local_def_id = self.state.def_map_builder.alloc_local_def(LocalDefData {
            module: module_id,
            name: name.clone(),
            kind,
            namespaces,
            visibility: visibility.clone(),
            source: self.item_source(generated_source, item_id),
            file_id: item.file_id,
            name_span: item.name_span,
            span: item.span,
            user_facing_attrs: item.user_facing_attrs,
        });
        self.state
            .def_map_builder
            .module_mut(module_id)
            .expect("module should exist for generated local definition")
            .local_defs
            .push(local_def_id);
        let def = DefId::Local(LocalDefRef {
            origin: DefMapRef::Crate(self.state.crate_ref),
            local_def: local_def_id,
        });
        // Update both the base scopes and the current snapshot. The base scopes make future import
        // refreshes see the generated name; the current snapshot lets later generated calls in this
        // pass resolve it immediately.
        let base_scope = self
            .state
            .base_scopes
            .get_mut(module_id.0)
            .expect("base scope should exist for generated local definition");
        let current_scope = self
            .current_scopes
            .module_scope_mut(self.state.crate_ref, module_id)
            .expect("current scope should exist for generated local definition");
        for namespace in namespaces.iter() {
            let binding = ScopeBinding::new(def, *visibilities.get(namespace), provenance);
            base_scope.insert_binding(&name, namespace, binding.clone());
            current_scope.insert_binding(&name, namespace, binding);
        }

        Some(local_def_id)
    }

    /// Record generated enum variants with the same namespace facts as source variants.
    fn collect_enum(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        enum_item: &rg_item_tree::EnumItem,
        generated_source: GeneratedSourceId,
        item_id: ItemTreeId,
    ) {
        let Some(local_def_id) = self.collect_named_def(
            module_id,
            item,
            generated_source,
            item_id,
            ScopeBindingProvenance::Direct,
        ) else {
            return;
        };
        let visibility = self
            .state
            .def_map_builder
            .resolve_visibility(module_id, &item.visibility);
        self.state.def_map_builder.alloc_local_enum_variants(
            module_id,
            local_def_id,
            enum_item,
            visibility,
            item.file_id,
        );
    }

    fn collect_macro_definition(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        macro_definition: &MacroDefinitionItem,
        generated_source: GeneratedSourceId,
        item_id: ItemTreeId,
        order: ItemOrder,
    ) {
        let provenance = match macro_definition {
            MacroDefinitionItem::MacroRules { attrs, .. }
                if self.macro_definition_is_exported(attrs) =>
            {
                ScopeBindingProvenance::DirectMacroExport
            }
            MacroDefinitionItem::MacroRules { .. } => ScopeBindingProvenance::DirectMacroRules,
            MacroDefinitionItem::MacroDef { .. } => ScopeBindingProvenance::Direct,
        };
        let Some(local_def_id) =
            self.collect_named_def(module_id, item, generated_source, item_id, provenance)
        else {
            return;
        };
        let Some(name) = item.name.clone() else {
            return;
        };

        self.state.macro_definitions.insert(
            local_def_id,
            MacroDefinitionRecord {
                order: order.clone(),
            },
        );
        if matches!(macro_definition, MacroDefinitionItem::MacroRules { .. }) {
            self.state.textual_macro_scopes.record_definition(
                module_id,
                name.clone(),
                local_def_id,
                order,
            );
        }
        if let MacroDefinitionItem::MacroRules { attrs, .. } = macro_definition
            && self.macro_definition_is_exported(attrs)
        {
            self.export_macro_definition_to_root(&name, local_def_id);
        }
        // Generated macro definitions inherit `$crate` from the macro that produced them, not from
        // the module where the generated definition is inserted.
        let dollar_crate = self.origin.dollar_crate.unwrap_or(self.state.crate_ref);
        self.state.def_map_builder.insert_macro_definition(
            local_def_id,
            MacroDefinitionData::from_item(
                macro_definition,
                item.docs.clone(),
                self.state.edition,
                dollar_crate,
            ),
        );
    }

    /// Updates both scope snapshots for a generated `#[macro_export]` definition.
    fn export_macro_definition_to_root(&mut self, name: &Name, local_def_id: LocalDefId) {
        let root_module = self.state.root_module;
        let binding = ScopeBinding::new(
            DefId::Local(LocalDefRef {
                origin: DefMapRef::Crate(self.state.crate_ref),
                local_def: local_def_id,
            }),
            Visibility::Public,
            ScopeBindingProvenance::MacroExport,
        );

        self.state
            .base_scopes
            .get_mut(root_module.0)
            .expect("root scope should exist before generated macro export collection")
            .insert_binding(name, Namespace::Macros, binding.clone());
        self.current_scopes
            .module_scope_mut(self.state.crate_ref, root_module)
            .expect("current root scope should exist for generated macro export")
            .insert_binding(name, Namespace::Macros, binding);
    }

    fn macro_definition_is_exported(&self, attrs: &MacroDefinitionAttrs) -> bool {
        if attrs.macro_export {
            return true;
        }

        let cfg = self.state.cfg_evaluator();
        attrs
            .cfg_attr_macro_export
            .iter()
            .any(|predicate| cfg.is_predicate_enabled(predicate))
    }

    fn collect_inline_module(
        &mut self,
        parent_module: ModuleId,
        item: &ItemNode,
        module_item: &ModuleItem,
        generated_source: GeneratedSourceId,
        order: ItemOrder,
    ) {
        let Some(module_name) = item.name.clone() else {
            return;
        };
        let ModuleSource::Inline { items } = &module_item.source else {
            // Out-of-line generated modules need call-site module file resolution. Skipping them is
            // a false negative, while inventing an empty module would create misleading scope data.
            return;
        };

        let visibility = item.visibility.clone();
        let semantic_visibility = self
            .state
            .def_map_builder
            .resolve_visibility(parent_module, &visibility);
        let child_module = self.state.def_map_builder.alloc_module(ModuleData {
            name: Some(module_name.clone()),
            name_span: item.name_span,
            docs: Documentation::concat(item.docs.clone(), module_item.inner_docs.clone()),
            user_facing_attrs: item.user_facing_attrs,
            visibility: semantic_visibility,
            parent: Some(parent_module),
            children: Vec::new(),
            local_defs: Vec::new(),
            impls: Vec::new(),
            imports: Vec::new(),
            unresolved_imports: Vec::new(),
            scope: ModuleScope::default(),
            origin: ModuleOrigin::Inline {
                declaration_file: item.file_id,
                declaration_span: item.span,
            },
        });
        // Inline generated modules extend all scope matrices in lockstep with the def-map module
        // arena so later generated children can be collected into the new module.
        self.state.base_scopes.push(Default::default());
        self.state
            .textual_macro_scopes
            .record_module_declaration(child_module, order.clone());
        self.current_scopes
            .push_module_scope(self.state.crate_ref, Default::default())
            .expect("current scopes should have a crate slot for generated module");
        self.state
            .def_map_builder
            .module_mut(parent_module)
            .expect("parent module should exist for generated child link")
            .children
            .push((module_name.clone(), child_module));
        let binding = ScopeBinding::new(
            DefId::Module(ModuleRef {
                origin: DefMapRef::Crate(self.state.crate_ref),
                module: child_module,
            }),
            semantic_visibility,
            ScopeBindingProvenance::Direct,
        );
        self.state
            .base_scopes
            .get_mut(parent_module.0)
            .expect("base scope should exist for generated child link")
            .insert_binding(&module_name, Namespace::Types, binding.clone());
        self.current_scopes
            .module_scope_mut(self.state.crate_ref, parent_module)
            .expect("current scope should exist for generated child link")
            .insert_binding(&module_name, Namespace::Types, binding);

        for (index, child_item) in items.iter().copied().enumerate() {
            self.collect_item(
                child_module,
                generated_source,
                child_item,
                order.generated_child(index),
            );
        }
    }

    fn collect_macro_call(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        macro_call: &MacroCallItem,
        order: ItemOrder,
    ) {
        // Macro-generated `include!("...")` would need a real file-relative expansion context to
        // resolve safely. The generated lowerer therefore records no builtin payload here.
        self.state.push_macro_call(
            MacroCallSite {
                module: module_id,
                source: self.origin.source,
                path: macro_call.path.clone(),
                callee: macro_call.callee.clone(),
                args: macro_call.args.clone(),
                builtin: macro_call.builtin.clone(),
                dollar_crate: self.origin.dollar_crate,
                file_id: item.file_id,
                span: item.span,
                order,
            },
            MacroCallOrigin::Generated {
                parent_call: self.origin.parent_call,
            },
        );
    }

    fn collect_local_impl(
        &mut self,
        module_id: ModuleId,
        item: &ItemNode,
        generated_source: GeneratedSourceId,
        item_id: ItemTreeId,
    ) {
        let local_impl_id = self.state.def_map_builder.alloc_local_impl(LocalImplData {
            module: module_id,
            source: self.item_source(generated_source, item_id),
            file_id: item.file_id,
            span: item.span,
        });
        self.state
            .def_map_builder
            .module_mut(module_id)
            .expect("module should exist for generated impl block")
            .impls
            .push(local_impl_id);
    }

    fn collect_use(&mut self, module_id: ModuleId, item: &ItemNode, use_item: &UseItem) {
        let imports: &[UseImport] = &use_item.imports;

        for (import_index, import) in imports.iter().enumerate() {
            let Some(mut path) = ImportPath::from_use_path(&import.path, self.origin.dollar_crate)
            else {
                continue;
            };
            if path.semantic().is_empty() {
                continue;
            }

            // The generated import's textual source is synthetic. Keep spans at the macro call site
            // so diagnostics and navigation have a real file location to point at.
            path.rebase(self.origin.span);
            let visibility = self
                .state
                .def_map_builder
                .resolve_visibility(module_id, &item.visibility);

            let import_id = self.state.def_map_builder.alloc_import(ImportData {
                module: module_id,
                visibility,
                kind: ImportKind::from_use_kind(import.kind),
                path,
                binding: ImportBinding::from_alias(&import.alias),
                alias_span: match &import.alias {
                    ImportAlias::Explicit { .. } => Some(self.origin.span),
                    ImportAlias::Inferred | ImportAlias::Hidden => None,
                },
                source: self.origin.source.into(),
                import_index,
                user_facing_attrs: item.user_facing_attrs,
            });
            self.state
                .def_map_builder
                .module_mut(module_id)
                .expect("module should exist for generated import")
                .imports
                .push(import_id);
        }
    }

    fn item_source(&self, generated_source: GeneratedSourceId, item: ItemTreeId) -> ItemSource {
        ItemSource::generated(
            self.origin.file_id,
            GeneratedItemRef {
                source: generated_source,
                item,
            },
        )
    }
}
