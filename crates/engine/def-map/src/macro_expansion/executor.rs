//! Runs macro expansion work outside fixed-point collection loops.
//!
//! Preparing expansion work is normally tied to mutable build state. Once the work is
//! self-contained, the matcher/transcriber step can run in a Rayon pool and return deterministic
//! outputs for callers to merge back into their own state.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context as _;
use rayon::prelude::*;

use rg_macro_expand::{DeclarativeMacro, ExpansionParseKind, ExpansionSyntax};
use rg_tt::{Span as TtSpan, TopSubtree};

use crate::build::DefMapPerformancePreference;

use super::cache::MacroExpansionCacheKey;

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
        if let Some(thread_limit) = Self::thread_limit(preference) {
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

    pub(crate) fn expand_jobs(&self, jobs: Vec<MacroExpansionJob>) -> Vec<MacroExpansionOutput> {
        self.thread_pool.install(|| {
            jobs.into_par_iter()
                .map(MacroExpansionJob::expand)
                .collect::<Vec<_>>()
        })
    }

    fn thread_limit(preference: DefMapPerformancePreference) -> Option<usize> {
        match preference {
            DefMapPerformancePreference::FasterBuilds => None,
            DefMapPerformancePreference::LowerPeakMemory => {
                Some(LOWER_PEAK_MEMORY_MACRO_EXPANSION_THREAD_LIMIT)
            }
        }
    }
}

/// A self-contained expansion job plus the caller-owned id used to merge the result.
pub(crate) struct MacroExpansionJob {
    pub(crate) id: usize,
    pub(crate) work: MacroExpansionWork,
}

impl MacroExpansionJob {
    fn expand(self) -> MacroExpansionOutput {
        self.work.expand(self.id)
    }
}

/// Result of expanding one macro call.
pub(crate) struct MacroExpansionOutput {
    pub(crate) id: usize,
    pub(crate) key: MacroExpansionCacheKey,
    pub(crate) macro_name: String,
    pub(crate) elapsed: Duration,
    pub(crate) generated_syntax: Option<ExpansionSyntax>,
}

/// Self-contained macro expansion job produced after resolution and cache lookup.
pub(crate) struct MacroExpansionWork {
    pub(super) key: MacroExpansionCacheKey,
    pub(super) macro_name: String,
    pub(super) macro_: Arc<DeclarativeMacro>,
    pub(super) args: TopSubtree,
    pub(super) call_site: TtSpan,
    pub(super) parse_kind: ExpansionParseKind,
}

impl MacroExpansionWork {
    fn expand(self, id: usize) -> MacroExpansionOutput {
        let started_at = Instant::now();
        let generated_syntax = self
            .macro_
            .expand_call_tokens(&self.args, self.call_site, self.parse_kind)
            .ok();
        let elapsed = started_at.elapsed();

        MacroExpansionOutput {
            id,
            key: self.key,
            macro_name: self.macro_name,
            elapsed,
            generated_syntax,
        }
    }
}
