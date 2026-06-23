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

use super::cache::MacroExpansionCacheKey;

const LOWER_PEAK_MEMORY_MACRO_EXPANSION_THREAD_LIMIT: usize = 2;

/// Build-time speed/memory preference for macro expansion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MacroExpansionPerformancePreference {
    /// Use unconstrained macro-expansion behavior.
    #[default]
    FasterBuilds,
    /// Bound bursty macro-expansion parallelism to reduce peak resident memory.
    LowerPeakMemory,
}

/// Dedicated pool for declarative macro expansion jobs.
pub struct MacroExpansionExecutor {
    thread_pool: rayon::ThreadPool,
}

impl MacroExpansionExecutor {
    pub fn new(preference: MacroExpansionPerformancePreference) -> anyhow::Result<Self> {
        // Macro expansion can allocate large parser and token-tree temporaries per worker. Keep
        // this pool optionally narrower than the global CPU pool so a few large expansions do not
        // multiply peak resident memory for the whole index build.
        let mut builder = rayon::ThreadPoolBuilder::new()
            .thread_name(|index| format!("rg-macro-runtime-expand-{index}"));
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

    pub fn expand_jobs(&self, jobs: Vec<MacroExpansionJob>) -> Vec<MacroExpansionOutput> {
        self.thread_pool.install(|| {
            jobs.into_par_iter()
                .map(MacroExpansionJob::expand)
                .collect::<Vec<_>>()
        })
    }

    fn thread_limit(preference: MacroExpansionPerformancePreference) -> Option<usize> {
        match preference {
            MacroExpansionPerformancePreference::FasterBuilds => None,
            MacroExpansionPerformancePreference::LowerPeakMemory => {
                Some(LOWER_PEAK_MEMORY_MACRO_EXPANSION_THREAD_LIMIT)
            }
        }
    }
}

/// A self-contained expansion job plus the caller-owned id used to merge the result.
pub struct MacroExpansionJob {
    pub id: usize,
    pub work: MacroExpansionWork,
}

impl MacroExpansionJob {
    fn expand(self) -> MacroExpansionOutput {
        let started_at = Instant::now();
        let syntax = self.work.expand_syntax();
        let elapsed = started_at.elapsed();

        MacroExpansionOutput {
            id: self.id,
            key: syntax.key,
            macro_name: syntax.macro_name,
            elapsed,
            generated_syntax: syntax.generated_syntax,
        }
    }
}

/// Result of expanding one macro call.
pub struct MacroExpansionOutput {
    pub id: usize,
    pub key: MacroExpansionCacheKey,
    pub macro_name: String,
    pub elapsed: Duration,
    pub generated_syntax: Option<ExpansionSyntax>,
}

/// Expanded syntax plus the cache key needed by both worker-pool and synchronous callers.
pub struct MacroExpansionSyntax {
    pub key: MacroExpansionCacheKey,
    pub macro_name: String,
    pub generated_syntax: Option<ExpansionSyntax>,
}

/// Self-contained macro expansion job produced after resolution and cache lookup.
pub struct MacroExpansionWork {
    pub(super) key: MacroExpansionCacheKey,
    pub(super) macro_name: String,
    pub(super) macro_: Arc<DeclarativeMacro>,
    pub(super) args: TopSubtree,
    pub(super) call_site: TtSpan,
    pub(super) parse_kind: ExpansionParseKind,
}

impl MacroExpansionWork {
    /// Run the matcher/transcriber step without adding executor ids or timing data.
    pub fn expand_syntax(self) -> MacroExpansionSyntax {
        let generated_syntax = self
            .macro_
            .expand_call_tokens(&self.args, self.call_site, self.parse_kind)
            .ok();

        MacroExpansionSyntax {
            key: self.key,
            macro_name: self.macro_name,
            generated_syntax,
        }
    }
}
