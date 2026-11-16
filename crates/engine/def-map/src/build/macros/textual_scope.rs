//! Source-order visibility for legacy `macro_rules!` definitions.
//!
//! Item-scope bindings answer which macros a module contains. Textual lookup additionally needs
//! to know which definitions precede a call, including where a child module was declared. Keep
//! that ordering separate from ordinary namespace lookup so deferred members can reuse it.

use std::collections::HashMap;

use rg_ir_model::{LocalDefId, ModuleId};
use rg_item_tree::MacroUseSelector;
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use super::ItemOrder;

/// Textual scope for `macro_rules!` definitions.
///
/// Unlike ordinary macro namespace bindings, textual `macro_rules!` visibility depends on source
/// order and on the declaration position of nested modules.
#[derive(Debug, Clone, Default, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub(crate) struct TextualMacroScopes {
    definitions: HashMap<ModuleId, HashMap<Name, Vec<TextualMacroDefinition>>>,
    module_declaration_orders: HashMap<ModuleId, ItemOrder>,
}

impl TextualMacroScopes {
    pub(crate) fn record_definition(
        &mut self,
        module: ModuleId,
        name: Name,
        local_def: LocalDefId,
        order: ItemOrder,
    ) {
        self.definitions
            .entry(module)
            .or_default()
            .entry(name)
            .or_default()
            .push(TextualMacroDefinition { local_def, order });
    }

    pub(crate) fn record_module_declaration(&mut self, module: ModuleId, order: ItemOrder) {
        self.module_declaration_orders.insert(module, order);
    }

    pub(crate) fn import_module_definitions(
        &mut self,
        target_module: ModuleId,
        source_module: ModuleId,
        order: ItemOrder,
        selector: &MacroUseSelector,
    ) {
        let Some(source_definitions) = self.definitions.get(&source_module) else {
            return;
        };

        // `#[macro_use] mod ...` is legacy surface area that we support as a practical shortcut:
        // macro_rules! definitions from the child become textual definitions in the parent at the
        // module declaration position. This matches the important valid-code behavior without
        // modeling every invalid intermediate state accepted or rejected by rustc.
        let mut imported = Vec::new();
        for (name, definitions) in source_definitions {
            if !selector.allows(name) {
                continue;
            }
            for definition in definitions {
                imported.push((name.clone(), definition.local_def));
            }
        }

        let target_definitions = self.definitions.entry(target_module).or_default();
        for (name, local_def) in imported {
            target_definitions
                .entry(name)
                .or_default()
                .push(TextualMacroDefinition {
                    local_def,
                    order: order.clone(),
                });
        }
    }

    pub(crate) fn module_declaration_order(&self, module: ModuleId) -> Option<&ItemOrder> {
        self.module_declaration_orders.get(&module)
    }

    pub(crate) fn latest_before(
        &self,
        module: ModuleId,
        name: &Name,
        boundary: &ItemOrder,
    ) -> Option<LocalDefId> {
        self.definitions
            .get(&module)?
            .get(name)?
            .iter()
            .filter(|definition| definition.order < *boundary)
            .max_by_key(|definition| &definition.order)
            .map(|definition| definition.local_def)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
struct TextualMacroDefinition {
    local_def: LocalDefId,
    order: ItemOrder,
}
