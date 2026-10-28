//! Bounds recursive item-macro expansion and records why calls were skipped.
//!
//! An invocation stops when its ancestry reaches the recursion limit. Other invocations keep
//! running. Once construction settles, collect only the stopped calls into a bounded report with
//! one ancestry example per macro identity.

use std::collections::HashMap;

use super::{MacroCallOrigin, MacroDirective, MacroDirectiveState};
use crate::MacroExpansionLimitGroup;

// Recursive generated macro calls can otherwise keep the fixed-point loop alive forever. Keep the
// cap high enough for real nested expansions while still bounding broken projects.
// TODO: Honor crate-level `#![recursion_limit]` attributes.
pub(crate) const MAX_MACRO_EXPANSION_DEPTH: usize = 128;

/// Bounded diagnostic assembled from invocations stopped by the recursion limit.
///
/// Calls are grouped by macro identity and keep at most one ancestry example per group. This makes
/// the report useful for recursive expansions without making its size follow the token tree or the
/// number of generated calls.
#[derive(Debug, Clone)]
pub(crate) struct PendingMacroExpansionLimitReport {
    pub(crate) groups: Vec<MacroExpansionLimitGroup>,
    pub(crate) omitted_call_count: usize,
}

impl PendingMacroExpansionLimitReport {
    /// Groups recursion-limited calls while consuming the report-wide group budget.
    pub(crate) fn collect(
        directives: &[MacroDirective],
        remaining_groups: &mut usize,
    ) -> Option<Self> {
        let mut groups = Vec::<PendingMacroExpansionLimitGroup>::new();
        let mut group_by_identity = HashMap::<&str, usize>::new();
        let mut omitted_call_count = 0;
        let mut limited_call_count = 0;

        for (call_id, directive) in directives.iter().enumerate() {
            if directive.state != MacroDirectiveState::RecursionLimitReached {
                continue;
            }
            limited_call_count += 1;
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

        (limited_call_count > 0).then_some(Self {
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
