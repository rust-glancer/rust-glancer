//! Applies reusable macro expansion jobs back to item-position attempts.
//!
//! Attempt collection is tied to target state and macro resolution. The prepared work is
//! self-contained, so it can run through the shared runtime before this module updates item macro
//! attempts and records def-map metrics.

use rg_macro_runtime::MacroExpansionRuntime;

use crate::profile::metric;

use super::MacroExpansionAttempt;

/// Executes all pending expansion work and writes the generated syntax back into each attempt.
pub(crate) fn expand_expansion_attempts(
    runtime: &mut MacroExpansionRuntime,
    attempts: &mut [MacroExpansionAttempt],
) -> anyhow::Result<()> {
    let work = attempts
        .iter_mut()
        .enumerate()
        .filter_map(|(attempt_id, attempt)| {
            attempt
                .take_pending_expansion()
                .map(|pending| (attempt_id, pending))
        })
        .collect::<Vec<_>>();

    if work.is_empty() {
        return Ok(());
    }

    // Expansion no longer borrows def-map state, so runtime can execute the expensive
    // matcher/transcriber work in parallel without making the collector itself concurrent.
    let timer = metric::TIMING_EXPAND_MACROS.start_timer();
    let mut results = runtime.expand_pending_batch(work)?;
    timer.finish();

    // Keep result application deterministic even though the work finished in parallel.
    results.sort_by_key(|result| result.id);
    for result in results {
        let syntax = result.generated_syntax;
        metric::EXPANSION_BY_NAME.record(&result.macro_name, result.elapsed);
        if syntax.is_some() {
            metric::MACRO_CALLS_EXPANDED.inc();
        } else {
            metric::MACRO_CALLS_FAILED.inc();
            metric::MACRO_EXPAND_FAILURES.inc();
            metric::FAILED_EXPAND_BY_NAME.inc(&result.macro_name);
        }

        let attempt = &mut attempts[result.id];
        attempt.set_expansion_result(syntax);
    }

    Ok(())
}
