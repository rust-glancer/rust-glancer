//! Expands item-position declarative macros during def-map construction.
//!
//! Macro expansion is tied to import resolution: a call may need imports to find its definition,
//! and its generated items may add new imports or new macros. This module keeps that loop local to
//! def-map by parsing expanded token trees into generated syntax and collecting those items into
//! the macro call's module.

use std::collections::HashMap;

use anyhow::Context as _;

use rg_item_tree::ItemTreeRef;
use rg_macro_expand::{Edition, ExpansionSyntax};
use rg_parse::{FileId, Span};
use rg_text::{Name, PackageNameInterners};
use rg_tt::{Span as TtSpan, TopSubtree, syntax_bridge::SpanFactory};
use rg_workspace::RustEdition;

use crate::{
    LocalDefId, ModuleId, ScopeBindingOrigin, TargetRef, query::path_resolution::PathResolutionEnv,
};

use super::{
    collect::TargetState,
    finalize::{FinalizeTargetStates, ScopeMatrix},
    stats::DefMapFinalizationStatsSink,
};

mod cache;
mod expand;
mod generated;
mod resolve;

use self::{
    cache::{MacroCompileRecord, MacroExpandRecord, PreparedMacroExpansion},
    expand::MacroExpansionWork,
    generated::{GeneratedCollector, GeneratedOrigin},
    resolve::{is_unsupported_builtin_macro_path, macro_path_from_text, resolve_macro_definition},
};

pub(super) use self::{
    cache::MacroExpansionCache,
    expand::{MacroExpansionExecutor, expand_expansion_attempts},
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
    /// The call resolved to a builtin/proc-like macro category outside this milestone.
    Unsupported,
}

#[derive(Debug, Clone)]
pub(super) struct MacroDefinitionRecord {
    pub(super) order: ItemOrder,
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

/// Marks the still-expandable macro calls as skipped after the fixed-point guard fires.
pub(super) fn mark_pending_macros_skipped_by_limit(
    states: &mut FinalizeTargetStates,
    stats: &mut DefMapFinalizationStatsSink<'_>,
) {
    let mut skipped = 0;

    for package_states in states.iter_dirty_mut() {
        for state in package_states {
            for directive in &mut state.macro_directives {
                if directive.state == MacroDirectiveState::Pending {
                    directive.state = MacroDirectiveState::Skipped;
                    skipped += 1;
                }
            }
        }
    }

    stats.record(|stats| stats.record_expansion_pass_limit_reached(skipped));
}

/// Selects which part of the macro worklist one expansion pass should inspect.
#[derive(Debug, Clone, Copy)]
pub(super) enum MacroExpansionScan<'a> {
    AllPending,
    NewCallsSince(&'a MacroExpansionCursors),
}

impl MacroExpansionScan<'_> {
    fn start_for(self, target: TargetRef) -> usize {
        match self {
            Self::AllPending => 0,
            Self::NewCallsSince(cursors) => cursors.start_for(target),
        }
    }
}

fn should_scan_directive(state: MacroDirectiveState, scan: MacroExpansionScan<'_>) -> bool {
    match state {
        MacroDirectiveState::Pending => true,
        // Unresolved calls are worth revisiting only after imports have been refreshed. Generated
        // items may introduce new imports or macro definitions, but repeated inner-loop scans see
        // the same scope snapshot and would only reproduce the same miss.
        MacroDirectiveState::Unresolved => matches!(scan, MacroExpansionScan::AllPending),
        MacroDirectiveState::Expanded
        | MacroDirectiveState::Failed
        | MacroDirectiveState::Skipped
        | MacroDirectiveState::Unsupported => false,
    }
}

/// Per-target cursors used when only macro calls generated by the previous pass should be scanned.
#[derive(Debug, Clone, Default)]
pub(super) struct MacroExpansionCursors {
    macro_call_counts: HashMap<TargetRef, usize>,
}

impl MacroExpansionCursors {
    pub(super) fn capture(states: &FinalizeTargetStates) -> Self {
        let mut macro_call_counts = HashMap::new();

        for package_states in states.iter_dirty() {
            for state in package_states {
                macro_call_counts.insert(state.target, state.macro_directives.len());
            }
        }

        Self { macro_call_counts }
    }

    fn start_for(&self, target: TargetRef) -> usize {
        self.macro_call_counts.get(&target).copied().unwrap_or(0)
    }
}

/// Resolves pending macro calls into concrete attempts for the current scope snapshot.
pub(super) fn collect_expansion_attempts(
    env: &impl PathResolutionEnv,
    states: &FinalizeTargetStates,
    scan: MacroExpansionScan<'_>,
    cache: &mut MacroExpansionCache,
    stats: &mut DefMapFinalizationStatsSink<'_>,
) -> anyhow::Result<Vec<MacroExpansionAttempt>> {
    let mut attempts = Vec::new();

    for package_states in states.iter_dirty() {
        for state in package_states {
            // A generated expansion pass only needs to inspect calls appended after the cursor.
            // Existing unresolved calls are revisited after imports have been refreshed.
            for (call_id, directive) in state
                .macro_directives
                .iter()
                .enumerate()
                .skip(scan.start_for(state.target))
            {
                if !should_scan_directive(directive.state, scan) {
                    continue;
                }

                let attempt = MacroExpansionAttempt::for_call(
                    env,
                    states,
                    cache,
                    state,
                    call_id,
                    &directive.call,
                )
                .with_context(|| {
                    format!(
                        "while attempting to inspect macro call in {}",
                        state.target_name
                    )
                })?;
                attempt.record(stats);
                attempts.push(attempt);
            }
        }
    }

    Ok(attempts)
}

/// Applies expansion results to target state and returns whether new scope facts were added.
pub(super) fn apply_expansion_attempts(
    states: &mut FinalizeTargetStates,
    interners: &mut PackageNameInterners,
    current_scopes: &mut ScopeMatrix,
    attempts: Vec<MacroExpansionAttempt>,
    stats: &mut DefMapFinalizationStatsSink<'_>,
) -> anyhow::Result<MacroExpansionApplyResult> {
    let mut result = MacroExpansionApplyResult::default();

    for attempt in attempts {
        let Some(state) = states.target_mut(attempt.target) else {
            continue;
        };

        let syntax = match attempt.outcome {
            MacroExpansionAttemptOutcome::NoSource(directive_state) => {
                if let Some(directive) = state.macro_directives.get_mut(attempt.call_id) {
                    directive.state = directive_state;
                }
                continue;
            }
            MacroExpansionAttemptOutcome::Generated(source) => source,
            MacroExpansionAttemptOutcome::PendingExpansion(_) => {
                unreachable!("macro expansion work should be executed before generated apply");
            }
        };
        let macro_name = attempt.macro_name;
        // Generated items are collected with the target's normal name interner so later stages see
        // the same names and source references as they do for ordinary items.
        let interner = interners
            .package_mut(attempt.target.package.0)
            .with_context(|| {
                format!(
                    "while attempting to fetch name interner for package {}",
                    attempt.target.package.0
                )
            })?;
        let collected = GeneratedCollector {
            state,
            interner,
            current_scopes,
            origin: attempt.origin,
            result: MacroExpansionApplyResult::default(),
        }
        .collect_syntax(syntax, macro_name.as_deref(), stats);
        let directive_state = match collected {
            Ok(collected) => {
                result.merge(collected);
                MacroDirectiveState::Expanded
            }
            Err(_) => MacroDirectiveState::Failed,
        };
        if let Some(directive) = state.macro_directives.get_mut(attempt.call_id) {
            directive.state = directive_state;
        }
    }

    Ok(result)
}

/// Whether applying a batch of expansion attempts changed the current def-map state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct MacroExpansionApplyResult {
    pub(super) changed: bool,
}

impl MacroExpansionApplyResult {
    fn mark_changed(&mut self) {
        self.changed = true;
    }

    fn merge(&mut self, other: Self) {
        self.changed |= other.changed;
    }
}

/// One inspected macro directive, including the work/result and stats events it produced.
pub(super) struct MacroExpansionAttempt {
    target: TargetRef,
    call_id: usize,
    macro_name: Option<String>,
    origin: GeneratedOrigin,
    outcome: MacroExpansionAttemptOutcome,
    record: MacroExpansionAttemptRecord,
}

impl MacroExpansionAttempt {
    fn skipped(target: TargetRef, call_id: usize, call: &MacroCallSite) -> Self {
        Self::new(
            target,
            call_id,
            call,
            call.path
                .clone()
                .or_else(|| call.callee.as_ref().map(ToString::to_string)),
            MacroExpansionAttemptOutcome::NoSource(MacroDirectiveState::Skipped),
            MacroExpansionAttemptRecord::skipped(),
        )
    }

    fn unsupported(
        target: TargetRef,
        call_id: usize,
        call: &MacroCallSite,
        path_text: &str,
    ) -> Self {
        Self::new(
            target,
            call_id,
            call,
            Some(path_text.to_string()),
            MacroExpansionAttemptOutcome::NoSource(MacroDirectiveState::Unsupported),
            MacroExpansionAttemptRecord::skipped(),
        )
    }

    fn unresolved(
        target: TargetRef,
        call_id: usize,
        call: &MacroCallSite,
        path_text: &str,
    ) -> Self {
        Self::new(
            target,
            call_id,
            call,
            Some(path_text.to_string()),
            MacroExpansionAttemptOutcome::NoSource(MacroDirectiveState::Unresolved),
            MacroExpansionAttemptRecord::unresolved(),
        )
    }

    fn resolved(
        target: TargetRef,
        call_id: usize,
        call: &MacroCallSite,
        path_text: &str,
        outcome: MacroExpansionAttemptOutcome,
        compile: MacroCompileRecord,
        expand: Option<MacroExpandRecord>,
    ) -> Self {
        Self::new(
            target,
            call_id,
            call,
            Some(path_text.to_string()),
            outcome,
            MacroExpansionAttemptRecord::resolved(compile, expand),
        )
    }

    fn resolved_skipped(
        target: TargetRef,
        call_id: usize,
        call: &MacroCallSite,
        path_text: &str,
    ) -> Self {
        Self::new(
            target,
            call_id,
            call,
            Some(path_text.to_string()),
            MacroExpansionAttemptOutcome::NoSource(MacroDirectiveState::Skipped),
            MacroExpansionAttemptRecord::resolved_skipped(),
        )
    }

    fn new(
        target: TargetRef,
        call_id: usize,
        call: &MacroCallSite,
        macro_name: Option<String>,
        outcome: MacroExpansionAttemptOutcome,
        record: MacroExpansionAttemptRecord,
    ) -> Self {
        Self {
            target,
            call_id,
            macro_name,
            origin: GeneratedOrigin {
                module: call.module,
                source: call.source,
                file_id: call.file_id,
                span: call.span,
                order: call.order.clone(),
                dollar_crate_target: None,
            },
            outcome,
            record,
        }
    }

    fn for_call(
        env: &impl PathResolutionEnv,
        states: &FinalizeTargetStates,
        cache: &mut MacroExpansionCache,
        state: &TargetState,
        call_id: usize,
        call: &MacroCallSite,
    ) -> anyhow::Result<Self> {
        // First normalize the syntactic call into a path and argument list. Calls that are not
        // item-position macro invocations are marked done so the worklist can move on.
        let Some(path_text) = call.path.as_deref().or_else(|| call.callee.as_deref()) else {
            return Ok(Self::skipped(state.target, call_id, call));
        };
        let Some(args) = call.args.as_ref() else {
            return Ok(Self::skipped(state.target, call_id, call));
        };
        let Some(path) = macro_path_from_text(path_text, call.dollar_crate_target) else {
            return Ok(Self::skipped(state.target, call_id, call));
        };

        // Then resolve against the current scope snapshot. Unresolved user macros stay resumable;
        // known builtins are unsupported for this milestone and are not retried.
        let Some(resolved) = resolve_macro_definition(env, states, state, call, &path)? else {
            if is_unsupported_builtin_macro_path(&path) {
                return Ok(Self::unsupported(state.target, call_id, call, path_text));
            }
            return Ok(Self::unresolved(state.target, call_id, call, path_text));
        };

        // Direct `macro_rules!` bindings cannot be used before a later definition in the same
        // module. Imported bindings and `#[macro_export]` root bindings are path-based and have
        // already gone through ordinary scope resolution.
        if resolved.origin == ScopeBindingOrigin::Direct
            && resolved.def_ref.target == state.target
            && resolved.local_def.module == call.module
            && let Some(order) = resolved.order
            && order > &call.order
        {
            return Ok(Self::resolved_skipped(
                state.target,
                call_id,
                call,
                path_text,
            ));
        }

        // Finally compile the definition and either reuse cached output or prepare self-contained
        // expansion work for the worker pool.
        let edition = macro_edition(resolved.data.edition);
        let compile_result = cache.compile(resolved.def_ref, resolved.data, edition);
        let Some(macro_) = compile_result.macro_ else {
            return Ok(Self::resolved(
                state.target,
                call_id,
                call,
                path_text,
                MacroExpansionAttemptOutcome::NoSource(MacroDirectiveState::Failed),
                compile_result.record,
                None,
            ));
        };

        let call_site =
            tt_span_for_parse_span(call.file_id, call.span, macro_edition(state.edition));
        let prepared_expansion =
            cache.prepare_expansion(resolved.def_ref, macro_, path_text, args, call_site);
        let outcome = match prepared_expansion.expansion {
            PreparedMacroExpansion::Syntax(syntax) => {
                MacroExpansionAttemptOutcome::Generated(syntax)
            }
            PreparedMacroExpansion::Failed => {
                MacroExpansionAttemptOutcome::NoSource(MacroDirectiveState::Failed)
            }
            PreparedMacroExpansion::Work(work) => {
                MacroExpansionAttemptOutcome::PendingExpansion(work)
            }
        };

        let mut attempt = Self::resolved(
            state.target,
            call_id,
            call,
            path_text,
            outcome,
            compile_result.record,
            Some(prepared_expansion.record),
        );
        attempt.origin.dollar_crate_target = Some(resolved.data.dollar_crate_target);
        Ok(attempt)
    }

    pub(super) fn needs_expansion(&self) -> bool {
        matches!(
            self.outcome,
            MacroExpansionAttemptOutcome::PendingExpansion(_)
        )
    }

    fn record(&self, stats: &mut DefMapFinalizationStatsSink<'_>) {
        let macro_name = self.macro_name.as_deref().unwrap_or("<unknown>");
        self.record.record(stats, macro_name);
    }

    fn take_expansion_work(&mut self) -> Option<MacroExpansionWork> {
        let outcome = std::mem::replace(
            &mut self.outcome,
            MacroExpansionAttemptOutcome::NoSource(MacroDirectiveState::Pending),
        );
        match outcome {
            MacroExpansionAttemptOutcome::PendingExpansion(work) => Some(work),
            other => {
                self.outcome = other;
                None
            }
        }
    }

    fn set_expansion_result(&mut self, syntax: Option<ExpansionSyntax>) {
        self.outcome = match syntax {
            Some(syntax) => MacroExpansionAttemptOutcome::Generated(syntax),
            None => MacroExpansionAttemptOutcome::NoSource(MacroDirectiveState::Failed),
        };
    }
}

/// Accounting events produced while classifying and preparing one macro attempt.
#[derive(Debug, Clone, Copy, Default)]
struct MacroExpansionAttemptRecord {
    resolved: bool,
    unresolved: bool,
    skipped: bool,
    compile: Option<MacroCompileRecord>,
    expand: Option<MacroExpandRecord>,
}

impl MacroExpansionAttemptRecord {
    fn skipped() -> Self {
        Self {
            skipped: true,
            ..Self::default()
        }
    }

    fn unresolved() -> Self {
        Self {
            unresolved: true,
            ..Self::default()
        }
    }

    fn resolved(compile: MacroCompileRecord, expand: Option<MacroExpandRecord>) -> Self {
        Self {
            resolved: true,
            compile: Some(compile),
            expand,
            ..Self::default()
        }
    }

    fn resolved_skipped() -> Self {
        Self {
            resolved: true,
            skipped: true,
            ..Self::default()
        }
    }

    fn record(&self, stats: &mut DefMapFinalizationStatsSink<'_>, macro_name: &str) {
        stats.record(|stats| {
            stats.macro_calls_seen += 1;

            if self.resolved {
                stats.macro_calls_resolved += 1;
            }
            if self.unresolved {
                stats.macro_calls_unresolved += 1;
                stats.record_unresolved_macro(macro_name);
            }
            if self.skipped {
                stats.macro_calls_skipped += 1;
            }

            if let Some(compile) = self.compile {
                match compile {
                    MacroCompileRecord::CacheHit { failed } => {
                        stats.macro_compile_cache_hits += 1;
                        if failed {
                            stats.record_compile_failure(macro_name);
                        }
                    }
                    MacroCompileRecord::Attempt { elapsed, failed } => {
                        stats.macro_compile_attempts += 1;
                        stats.timings.compile_macros += elapsed;
                        if failed {
                            stats.record_compile_failure(macro_name);
                        }
                    }
                }
            }

            if let Some(expand) = self.expand {
                match expand {
                    MacroExpandRecord::CacheHit { failed } => {
                        stats.macro_expand_cache_hits += 1;
                        if failed {
                            stats.record_expand_failure(macro_name);
                        } else {
                            stats.macro_calls_expanded += 1;
                        }
                    }
                    MacroExpandRecord::Attempt => {
                        stats.macro_expand_attempts += 1;
                    }
                }
            }
        });
    }
}

enum MacroExpansionAttemptOutcome {
    NoSource(MacroDirectiveState),
    Generated(ExpansionSyntax),
    PendingExpansion(MacroExpansionWork),
}

fn macro_edition(edition: RustEdition) -> Edition {
    match edition {
        RustEdition::Edition2015 => Edition::Edition2015,
        RustEdition::Edition2018 => Edition::Edition2018,
        RustEdition::Edition2021 => Edition::Edition2021,
        RustEdition::Edition2024 => Edition::Edition2024,
    }
}

fn tt_span_for_parse_span(file_id: FileId, span: Span, edition: Edition) -> TtSpan {
    let text_range = rg_syntax::TextRange::new(span.text.start.into(), span.text.end.into());
    SpanFactory::new(
        u32::try_from(file_id.0).expect("file id should fit macro span storage"),
        edition,
    )
    .span_for(text_range)
}
