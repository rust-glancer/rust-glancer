//! Runs macro expansion work outside the main def-map finalization loop.
//!
//! Preparing attempts is still single-threaded because it touches target state and caches. Once the
//! work is self-contained, expansion can run in a Rayon pool and then merge results back in call
//! order.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context as _;
use rayon::prelude::*;

use rg_macro_expand::{DeclarativeMacro, ExpansionSyntax};
use rg_tt::{Span as TtSpan, TopSubtree};

use super::{
    MacroExpansionAttempt,
    cache::{MacroExpansionCache, MacroExpansionCacheKey},
};
use crate::build::stats::DefMapFinalizationStatsSink;

/// Dedicated pool for declarative macro expansion jobs.
pub(crate) struct MacroExpansionExecutor {
    thread_pool: rayon::ThreadPool,
}

impl MacroExpansionExecutor {
    pub(crate) fn new() -> anyhow::Result<Self> {
        let thread_pool = rayon::ThreadPoolBuilder::new()
            .thread_name(|index| format!("rg-def-map-macro-expand-{index}"))
            .build()
            .context("while attempting to create macro expansion thread pool")?;

        Ok(Self { thread_pool })
    }
}

/// Executes all pending expansion work and writes the generated syntax back into each attempt.
pub(crate) fn expand_expansion_attempts(
    executor: &MacroExpansionExecutor,
    attempts: &mut [MacroExpansionAttempt],
    cache: &mut MacroExpansionCache,
    stats: &mut DefMapFinalizationStatsSink<'_>,
) {
    let work = attempts
        .iter_mut()
        .enumerate()
        .filter_map(|(attempt_id, attempt)| {
            attempt.take_expansion_work().map(|work| (attempt_id, work))
        })
        .collect::<Vec<_>>();

    if work.is_empty() {
        return;
    }

    // Expansion no longer borrows def-map state, so the expensive matcher/transcriber work can run
    // in parallel without making the collector itself concurrent.
    let timer = stats.start_timer();
    let mut results = executor.thread_pool.install(|| {
        work.into_par_iter()
            .map(|(attempt_id, work)| work.expand(attempt_id))
            .collect::<Vec<_>>()
    });
    stats.finish_timer(timer, |timings, elapsed| {
        timings.expand_macros += elapsed;
    });

    // Keep result application deterministic even though the work finished in parallel.
    results.sort_by_key(|result| result.attempt_id);
    for result in results {
        let syntax = result.generated_syntax;
        cache.insert_expansion(result.key, syntax.clone());
        stats.record(|stats| {
            stats.record_macro_expansion_elapsed(&result.macro_name, result.elapsed);
            if syntax.is_some() {
                stats.macro_calls_expanded += 1;
            } else {
                stats.record_expand_failure(&result.macro_name);
            }
        });

        let attempt = &mut attempts[result.attempt_id];
        attempt.set_expansion_result(syntax);
    }
}

/// Self-contained macro expansion job produced after resolution and cache lookup.
pub(super) struct MacroExpansionWork {
    pub(super) key: MacroExpansionCacheKey,
    pub(super) macro_name: String,
    pub(super) macro_: Arc<DeclarativeMacro>,
    pub(super) args: TopSubtree,
    pub(super) call_site: TtSpan,
}

impl MacroExpansionWork {
    fn expand(self, attempt_id: usize) -> MacroExpansionWorkResult {
        let started_at = Instant::now();
        let generated_syntax = self
            .macro_
            .expand_call_tokens(&self.args, self.call_site)
            .ok();
        let elapsed = started_at.elapsed();

        MacroExpansionWorkResult {
            attempt_id,
            key: self.key,
            macro_name: self.macro_name,
            elapsed,
            generated_syntax,
        }
    }
}

struct MacroExpansionWorkResult {
    attempt_id: usize,
    key: MacroExpansionCacheKey,
    macro_name: String,
    elapsed: Duration,
    generated_syntax: Option<ExpansionSyntax>,
}
