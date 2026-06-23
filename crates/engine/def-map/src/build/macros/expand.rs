//! Applies reusable macro expansion jobs back to item-position attempts.
//!
//! Attempt collection is tied to target state and macro resolution. The prepared work is
//! self-contained, so it can run through the shared executor before this module updates item macro
//! attempts and records def-map metrics.

use rg_macro_runtime::{MacroExpansionCache, MacroExpansionExecutor, MacroExpansionJob};

use crate::profile::metric;

use super::MacroExpansionAttempt;

/// Executes all pending expansion work and writes the generated syntax back into each attempt.
pub(crate) fn expand_expansion_attempts(
    executor: &MacroExpansionExecutor,
    attempts: &mut [MacroExpansionAttempt],
    cache: &mut MacroExpansionCache,
) {
    let work = attempts
        .iter_mut()
        .enumerate()
        .filter_map(|(attempt_id, attempt)| {
            attempt.take_expansion_work().map(|work| MacroExpansionJob {
                id: attempt_id,
                work,
            })
        })
        .collect::<Vec<_>>();

    if work.is_empty() {
        return;
    }

    // Expansion no longer borrows def-map state, so the expensive matcher/transcriber work can run
    // in parallel without making the collector itself concurrent.
    let timer = metric::TIMING_EXPAND_MACROS.start_timer();
    let mut results = executor.expand_jobs(work);
    timer.finish();

    // Keep result application deterministic even though the work finished in parallel.
    results.sort_by_key(|result| result.id);
    for result in results {
        let syntax = result.generated_syntax;
        cache.insert_expansion(result.key, syntax.clone());
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
}
