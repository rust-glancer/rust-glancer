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

/// Dedicated pool for declarative macro expansion jobs.
pub(crate) struct MacroExpansionExecutor {
    thread_pool: rayon::ThreadPool,
}

impl MacroExpansionExecutor {
    pub(crate) fn new() -> anyhow::Result<Self> {
        let thread_pool = rayon::ThreadPoolBuilder::new()
            .thread_name(|index| format!("rg-macro-runtime-expand-{index}"))
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
}

/// A self-contained expansion job plus the caller-owned id used to merge the result.
pub(crate) struct MacroExpansionJob {
    pub(crate) id: usize,
    pub(crate) work: MacroExpansionWork,
}

impl MacroExpansionJob {
    pub(crate) fn expand(self) -> MacroExpansionOutput {
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
pub(crate) struct MacroExpansionOutput {
    pub(crate) id: usize,
    pub(crate) key: MacroExpansionCacheKey,
    pub(crate) macro_name: String,
    pub(crate) elapsed: Duration,
    pub(crate) generated_syntax: Option<ExpansionSyntax>,
}

/// Expanded syntax plus the cache key needed by both worker-pool and synchronous callers.
pub(crate) struct MacroExpansionSyntax {
    pub(crate) key: MacroExpansionCacheKey,
    pub(crate) macro_name: String,
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
    /// Run the matcher/transcriber step without adding executor ids or timing data.
    pub(crate) fn expand_syntax(self) -> MacroExpansionSyntax {
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
