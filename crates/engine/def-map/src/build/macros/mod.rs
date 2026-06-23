//! Expands item-position declarative macros during def-map construction.
//!
//! Macro expansion is tied to import resolution: a call may need imports to find its definition,
//! and its generated items may add new imports or new macros. This module keeps that loop local to
//! def-map by parsing expanded token trees into generated syntax and collecting those items into
//! the macro call's module.

use std::collections::HashMap;

use rg_ir_model::{LocalDefId, ModuleId, ModuleRef, TargetRef};
use rg_item_tree::{BuiltinMacroItem, ItemTreeRef, MacroUseSelector};
use rg_parse::{FileId, Span};
use rg_text::Name;
use rg_tt::TopSubtree;

use crate::profile::metric;

use super::finalize::FinalizeTargetStates;

mod attempts;
mod expand;
mod generated;
mod generated_tree;
mod resolve;
mod source_fragment;

pub(super) use rg_macro_runtime::{MacroExpansionCache, MacroExpansionExecutor};

pub(super) use self::{
    attempts::{
        MacroExpansionApplyResult, MacroExpansionAttempt, MacroExpansionCursors,
        MacroExpansionScan, apply_expansion_attempts, collect_expansion_attempts,
    },
    expand::expand_expansion_attempts,
};

// Recursive generated macro calls can otherwise keep the fixed-point loop alive forever. Keep the
// cap high enough for real nested expansions while still bounding broken projects.
pub(super) const MAX_MACRO_EXPANSION_PASSES: usize = 128;

#[derive(Debug, Clone)]
pub(super) struct MacroDirective {
    pub(super) call: MacroCallSite,
    pub(super) state: MacroDirectiveState,
}

/// Worklist state for an item-position macro call seen during def-map construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MacroDirectiveState {
    /// The call has not yet been resolved against the current scope snapshot.
    Pending,
    /// Resolution failed, but a later import refresh may make the macro visible.
    Unresolved,
    /// Expansion succeeded and generated items have been collected.
    Expanded,
    /// Compilation, expansion, or generated-source parsing failed.
    Failed,
    /// The call is known not to be expandable by this engine.
    Skipped,
    /// The call names a builtin that cannot contribute def-map items.
    IgnoredByDefMap,
    /// The call names a macro category that would need dedicated support to model correctly.
    Unsupported,
}

#[derive(Debug, Clone)]
pub(super) struct MacroDefinitionRecord {
    pub(super) order: ItemOrder,
}

/// Legacy `#[macro_use] extern crate ...` request used by unqualified macro fallback lookup.
///
/// We intentionally model this as a small compatibility bridge: exported root macros from the
/// source crate are consulted only after textual and ordinary module-scope lookup fail. That covers
/// the common legacy dependency pattern without turning the current macro engine into a full Rust
/// 2015 macro prelude model.
pub(super) struct MacroUseImport {
    pub(super) module: ModuleId,
    pub(super) source_module: ModuleRef,
    pub(super) selector: MacroUseSelector,
}

/// Build-time textual scope for `macro_rules!` definitions.
///
/// Unlike ordinary macro namespace bindings, textual `macro_rules!` visibility depends on source
/// order and on the declaration position of nested modules. We keep that ordering state only while
/// expanding macros; generated items are collected into the frozen def-map afterwards.
#[derive(Debug, Clone, Default)]
pub(super) struct TextualMacroScopes {
    definitions: HashMap<ModuleId, HashMap<Name, Vec<TextualMacroDefinition>>>,
    module_declaration_orders: HashMap<ModuleId, ItemOrder>,
}

impl TextualMacroScopes {
    pub(super) fn record_definition(
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

    pub(super) fn record_module_declaration(&mut self, module: ModuleId, order: ItemOrder) {
        self.module_declaration_orders.insert(module, order);
    }

    pub(super) fn import_module_definitions(
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

    fn module_declaration_order(&self, module: ModuleId) -> Option<&ItemOrder> {
        self.module_declaration_orders.get(&module)
    }

    fn latest_before(
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

#[derive(Debug, Clone)]
struct TextualMacroDefinition {
    local_def: LocalDefId,
    order: ItemOrder,
}

#[derive(Debug, Clone)]
pub(super) struct MacroCallSite {
    pub(super) module: ModuleId,
    pub(super) source: ItemTreeRef,
    pub(super) path: Option<String>,
    pub(super) callee: Option<Name>,
    pub(super) args: Option<TopSubtree>,
    pub(super) builtin: Option<BuiltinMacroItem>,
    pub(super) dollar_crate_target: Option<TargetRef>,
    pub(super) file_id: FileId,
    pub(super) span: Span,
    pub(super) order: ItemOrder,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ItemOrder(Vec<usize>);

impl ItemOrder {
    pub(super) fn real(index: usize) -> Self {
        Self(vec![index])
    }

    pub(super) fn generated_child(&self, index: usize) -> Self {
        let mut order = self.0.clone();
        order.push(index);
        Self(order)
    }
}

/// Marks the still-retryable macro calls as skipped after the fixed-point guard fires.
pub(super) fn mark_retryable_macros_skipped_by_limit(states: &mut FinalizeTargetStates) {
    let mut skipped = 0;

    for package_states in states.iter_dirty_mut() {
        for state in package_states {
            for directive in &mut state.macro_directives {
                if matches!(
                    directive.state,
                    MacroDirectiveState::Pending | MacroDirectiveState::Unresolved
                ) {
                    directive.state = MacroDirectiveState::Skipped;
                    skipped += 1;
                }
            }
        }
    }

    metric::EXPANSION_PASS_LIMIT_REACHED.record_bool(true);
    metric::MACRO_CALLS_SKIPPED.add(skipped as u64);
    metric::MACRO_CALLS_SKIPPED_BY_LIMIT.add(skipped as u64);
}
