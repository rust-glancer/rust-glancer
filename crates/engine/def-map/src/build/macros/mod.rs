//! Expands item-shaped declarative macros during def-map construction.
//!
//! Macro expansion is tied to import resolution: a call may need imports to find its definition,
//! and its generated items may add new imports or new macros. This module keeps that loop local to
//! def-map by parsing expanded token trees into generated syntax and splicing the result according
//! to the call's placement. Module-position output enters the caller's module; associated output
//! replaces the invocation inside its trait or impl, and nested calls preserve that owner slot.
//!
//! For example, `make_types!();` at module scope contributes ordinary module items, while
//! `impl User { make_methods!(); }` contributes methods or associated consts to that `impl User`.
//! If `make_methods!` expands to another macro call, that nested call stays in the same impl slot.
//!
//! Most generated items can be spliced immediately. A generated `mod child;` still needs its child
//! file, while a generated `include!(...)` needs a real file whose items retain the include call's
//! placement. For those two source edges, the collector keeps the corresponding semantic
//! continuation and emits a project-owned lookup request. Project construction captures and lowers
//! the file, then resumes this same expansion/finalization session.

use std::{collections::HashMap, sync::Arc};

use rg_ir_model::{CrateRef, LocalDefId, ModuleId, ModuleRef};
use rg_item_tree::{BuiltinMacroItem, ItemTreeRef, MacroUseSelector};
use rg_parse::{FileId, ModuleFileContext, Span};
use rg_text::Name;
use rg_tt::TopSubtree;

use crate::{MacroExpansionLimitGroup, profile::metric};

use super::finalize::FinalizeCrateStates;

mod attempts;
mod expand;
mod generated;
mod generated_tree;
mod resolve;
mod source_fragment;

pub(super) use self::generated::{PendingGeneratedInclude, PendingGeneratedModule};
pub(super) use self::{
    attempts::{
        MacroExpansionApplyResult, MacroExpansionAttempt, MacroExpansionCursors,
        MacroExpansionScan, apply_expansion_attempts, collect_expansion_attempts,
    },
    expand::expand_expansion_attempts,
    generated::apply_pending_macro_source_files,
};

// Recursive generated macro calls can otherwise keep the fixed-point loop alive forever. Keep the
// cap high enough for real nested expansions while still bounding broken projects.
pub(super) const MAX_MACRO_EXPANSION_PASSES: usize = 128;

#[derive(Debug, Clone)]
pub(super) struct MacroDirective {
    pub(super) call: MacroCallSite,
    pub(super) origin: MacroCallOrigin,
    pub(super) state: MacroDirectiveState,
}

impl MacroDirective {
    fn is_retryable(&self) -> bool {
        matches!(
            self.state,
            MacroDirectiveState::Pending | MacroDirectiveState::Unresolved
        )
    }
}

/// Says whether a queued macro call came from source or from another queued call.
///
/// Generated calls point back into the same append-only directive list. That one index is enough
/// to reconstruct a diagnostic chain without retaining a second expansion graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MacroCallOrigin {
    Source,
    Generated { parent_call: usize },
}

/// Says where a successful item-shaped expansion is spliced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MacroCallPlacement {
    /// Ordinary module-position output, such as `make_types!();`, enters the caller's module.
    ModuleItems,
    /// Output from `impl User { make_methods!(); }` replaces the call inside that trait or impl.
    AssociatedItems { call_source: crate::ItemSource },
}

/// Bounded diagnostic assembled before retryable macro calls are marked as skipped.
///
/// Calls are grouped by macro identity and keep at most one ancestry example per group. This makes
/// the report useful for recursive expansions without making its size follow the token tree or the
/// number of generated calls.
#[derive(Debug, Clone)]
pub(super) struct PendingMacroExpansionLimitReport {
    pub(super) groups: Vec<MacroExpansionLimitGroup>,
    pub(super) omitted_call_count: usize,
}

impl PendingMacroExpansionLimitReport {
    /// Groups retryable calls while consuming the report-wide group budget.
    fn collect(directives: &[MacroDirective], remaining_groups: &mut usize) -> Option<Self> {
        let mut groups = Vec::<PendingMacroExpansionLimitGroup>::new();
        let mut group_by_identity = HashMap::<&str, usize>::new();
        let mut omitted_call_count = 0;
        let mut retryable_call_count = 0;

        for (call_id, directive) in directives.iter().enumerate() {
            if !directive.is_retryable() {
                continue;
            }
            retryable_call_count += 1;
            let macro_name = directive.call.identity();
            let generated = matches!(directive.origin, MacroCallOrigin::Generated { .. });

            // Repeated calls add counts to one identity. For generated calls, retain the longest
            // ancestry call id because it is usually the most useful explanation of recursion.
            // The chain itself is rendered only after every candidate has been considered.
            if let Some(group_id) = group_by_identity.get(macro_name).copied() {
                let group = &mut groups[group_id];
                group.skipped_call_count += 1;
                if generated {
                    group.generated_call_count += 1;
                    let candidate_depth = Self::visit_ancestry(directives, call_id, |_| {}).0;
                    if candidate_depth > group.example_depth {
                        group.example_call = call_id;
                        group.example_depth = candidate_depth;
                    }
                } else {
                    group.source_call_count += 1;
                }
                continue;
            }

            // The budget is shared by every affected crate in this build. Once it is exhausted,
            // count unrepresented calls without allocating more rendered groups and chains.
            if *remaining_groups == 0 {
                omitted_call_count += 1;
                continue;
            }
            *remaining_groups -= 1;
            let group_id = groups.len();
            groups.push(PendingMacroExpansionLimitGroup {
                macro_name: macro_name.to_string(),
                skipped_call_count: 1,
                source_call_count: usize::from(!generated),
                generated_call_count: usize::from(generated),
                example_call: call_id,
                example_depth: Self::visit_ancestry(directives, call_id, |_| {}).0,
            });
            group_by_identity.insert(macro_name, group_id);
        }

        (retryable_call_count > 0).then_some(Self {
            groups: groups
                .into_iter()
                .map(|group| group.finish(directives))
                .collect(),
            omitted_call_count,
        })
    }

    /// Visits one bounded ancestry from leaf to source without allocating rendered identities.
    fn visit_ancestry(
        directives: &[MacroDirective],
        call_id: usize,
        mut visit: impl FnMut(&MacroDirective),
    ) -> (usize, bool) {
        const MAX_CHAIN_DEPTH: usize = 12;

        let mut depth = 0;
        let mut current = call_id;
        let mut truncated = false;
        while let Some(directive) = directives.get(current) {
            visit(directive);
            depth += 1;
            let MacroCallOrigin::Generated { parent_call } = directive.origin else {
                break;
            };
            if depth == MAX_CHAIN_DEPTH {
                truncated = true;
                break;
            }
            if parent_call >= current {
                // Generated calls are appended after their parent. Treat malformed ancestry as a
                // boundary rather than risking a diagnostic loop.
                truncated = true;
                break;
            }
            current = parent_call;
        }
        (depth, truncated)
    }
}

/// Allocation-light group state retained while report candidates are still being compared.
struct PendingMacroExpansionLimitGroup {
    macro_name: String,
    skipped_call_count: usize,
    source_call_count: usize,
    generated_call_count: usize,
    example_call: usize,
    example_depth: usize,
}

impl PendingMacroExpansionLimitGroup {
    /// Renders the selected ancestry once, after no later call can replace it.
    fn finish(self, directives: &[MacroDirective]) -> MacroExpansionLimitGroup {
        let mut example_chain = Vec::with_capacity(self.example_depth);
        let (_, chain_truncated) = PendingMacroExpansionLimitReport::visit_ancestry(
            directives,
            self.example_call,
            |directive| example_chain.push(directive.call.identity().to_string()),
        );
        example_chain.reverse();

        MacroExpansionLimitGroup {
            macro_name: self.macro_name,
            skipped_call_count: self.skipped_call_count,
            source_call_count: self.source_call_count,
            generated_call_count: self.generated_call_count,
            example_chain,
            chain_truncated,
        }
    }
}

/// Worklist state for an item-shaped macro call seen during def-map construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MacroDirectiveState {
    /// The call has not yet been resolved against the current scope snapshot.
    Pending,
    /// Resolution failed, but a later import refresh may make the macro visible.
    Unresolved,
    /// Expansion succeeded and its output was spliced according to the call placement.
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
/// expanding macros; generated module items and associated replacement edges enter the frozen
/// def-map afterwards.
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

/// One queued item-shaped macro call plus the call-site state needed after ItemTree lowering.
///
/// `module` supplies the scope used to resolve the call and any nested macros, while `placement`
/// says whether successful output enters that module or replaces an associated-item call. The
/// `module_file_context` also follows the caller: a macro imported from another crate must still
/// resolve a generated `mod child;` next to the invocation, not next to its definition.
#[derive(Debug, Clone)]
pub(super) struct MacroCallSite {
    pub(super) module: ModuleId,
    pub(super) source: ItemTreeRef,
    pub(super) path: Option<String>,
    pub(super) callee: Option<Name>,
    pub(super) args: Option<TopSubtree>,
    pub(super) builtin: Option<BuiltinMacroItem>,
    pub(super) dollar_crate: Option<CrateRef>,
    pub(super) file_id: FileId,
    pub(super) span: Span,
    pub(super) order: ItemOrder,
    /// Destination retained by nested calls and source-backed continuations.
    pub(super) placement: MacroCallPlacement,
    /// Logical filesystem base inherited by declarations emitted from this call.
    pub(super) module_file_context: Arc<ModuleFileContext>,
}

impl MacroCallSite {
    fn identity(&self) -> &str {
        self.path
            .as_deref()
            .or_else(|| self.callee.as_ref().map(Name::as_str))
            .unwrap_or("<unknown>")
    }
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

/// Captures a bounded report, then marks retryable calls as skipped after the guard fires.
pub(super) fn mark_retryable_macros_skipped_by_limit(states: &mut FinalizeCrateStates) {
    let mut skipped = 0;
    let mut remaining_groups = 64;

    for package_states in states.iter_dirty_mut() {
        for state in package_states {
            state.macro_expansion_limit = PendingMacroExpansionLimitReport::collect(
                &state.macro_directives,
                &mut remaining_groups,
            );
            for directive in &mut state.macro_directives {
                if directive.is_retryable() {
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
