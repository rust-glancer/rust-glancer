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
use crate::{build::DefMapPerformancePreference, profile::metric};

const LOWER_PEAK_MEMORY_MACRO_EXPANSION_THREAD_LIMIT: usize = 2;

/// Dedicated pool for declarative macro expansion jobs.
pub(crate) struct MacroExpansionExecutor {
    thread_pool: rayon::ThreadPool,
}

impl MacroExpansionExecutor {
    pub(crate) fn new(preference: DefMapPerformancePreference) -> anyhow::Result<Self> {
        // Macro expansion can allocate large parser and token-tree temporaries per worker. Keep
        // this pool optionally narrower than the global CPU pool so a few large expansions do not
        // multiply peak resident memory for the whole index build.
        let mut builder = rayon::ThreadPoolBuilder::new()
            .thread_name(|index| format!("rg-def-map-macro-expand-{index}"));
        if let Some(thread_limit) = macro_expansion_thread_limit(preference) {
            let worker_count = std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(thread_limit)
                .min(thread_limit);
            builder = builder.num_threads(worker_count);
        }
        let thread_pool = builder
            .build()
            .context("while attempting to create macro expansion thread pool")?;

        Ok(Self { thread_pool })
    }
}

fn macro_expansion_thread_limit(preference: DefMapPerformancePreference) -> Option<usize> {
    match preference {
        DefMapPerformancePreference::FasterBuilds => None,
        DefMapPerformancePreference::LowerPeakMemory => {
            Some(LOWER_PEAK_MEMORY_MACRO_EXPANSION_THREAD_LIMIT)
        }
    }
}

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
            attempt.take_expansion_work().map(|work| (attempt_id, work))
        })
        .collect::<Vec<_>>();

    if work.is_empty() {
        return;
    }

    // Expansion no longer borrows def-map state, so the expensive matcher/transcriber work can run
    // in parallel without making the collector itself concurrent.
    let timer = metric::TIMING_EXPAND_MACROS.start_timer();
    let mut results = executor.thread_pool.install(|| {
        work.into_par_iter()
            .map(|(attempt_id, work)| work.expand(attempt_id))
            .collect::<Vec<_>>()
    });
    timer.finish();

    // Keep result application deterministic even though the work finished in parallel.
    results.sort_by_key(|result| result.attempt_id);
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
