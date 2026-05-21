use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

/// Counters and timings for the def-map fixed point that resolves imports and macros.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DefMapFinalizationStats {
    pub rounds: usize,
    pub expansion_passes: usize,
    pub expansion_pass_limit: usize,
    pub expansion_pass_limit_reached: bool,
    pub macro_calls_seen: usize,
    pub macro_calls_resolved: usize,
    pub macro_calls_unresolved: usize,
    pub macro_calls_skipped: usize,
    pub macro_calls_skipped_by_limit: usize,
    pub macro_calls_expanded: usize,
    pub macro_calls_failed: usize,
    pub macro_compile_failures: usize,
    pub macro_expand_failures: usize,
    pub macro_compile_attempts: usize,
    pub macro_compile_cache_hits: usize,
    pub macro_expand_attempts: usize,
    pub macro_expand_cache_hits: usize,
    pub generated_sources_parsed: usize,
    pub generated_source_parse_failures: usize,
    pub generated_items_seen: usize,
    pub failed_macros: BTreeMap<String, MacroExpansionFailureStats>,
    pub unresolved_macros: BTreeMap<String, usize>,
    pub slow_macros: BTreeMap<String, MacroExpansionSlowStats>,
    pub timings: DefMapFinalizationTimingStats,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MacroExpansionFailureStats {
    pub compile_failed: usize,
    pub expand_failed: usize,
    pub parse_failed: usize,
}

impl MacroExpansionFailureStats {
    pub fn total(&self) -> usize {
        self.compile_failed + self.expand_failed + self.parse_failed
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MacroExpansionSlowStats {
    pub count: usize,
    pub total: Duration,
    pub max: Duration,
}

impl MacroExpansionSlowStats {
    pub fn average(&self) -> Duration {
        if self.count == 0 {
            return Duration::ZERO;
        }

        self.total / u32::try_from(self.count).unwrap_or(u32::MAX)
    }
}

impl DefMapFinalizationStats {
    pub(super) fn record_unresolved_macro(&mut self, macro_name: &str) {
        *self
            .unresolved_macros
            .entry(macro_name.to_string())
            .or_default() += 1;
    }

    pub(super) fn record_compile_failure(&mut self, macro_name: &str) {
        self.macro_calls_failed += 1;
        self.macro_compile_failures += 1;
        self.failed_macros
            .entry(macro_name.to_string())
            .or_default()
            .compile_failed += 1;
    }

    pub(super) fn record_expand_failure(&mut self, macro_name: &str) {
        self.macro_calls_failed += 1;
        self.macro_expand_failures += 1;
        self.failed_macros
            .entry(macro_name.to_string())
            .or_default()
            .expand_failed += 1;
    }

    pub(super) fn record_generated_source_parse_failure(&mut self, macro_name: &str) {
        self.macro_calls_failed += 1;
        self.generated_source_parse_failures += 1;
        self.failed_macros
            .entry(macro_name.to_string())
            .or_default()
            .parse_failed += 1;
    }

    pub(super) fn record_macro_expansion_elapsed(&mut self, macro_name: &str, elapsed: Duration) {
        let stats = self.slow_macros.entry(macro_name.to_string()).or_default();
        stats.count += 1;
        stats.total += elapsed;
        stats.max = stats.max.max(elapsed);
    }

    pub(super) fn record_expansion_pass_limit_reached(&mut self, skipped: usize) {
        self.expansion_pass_limit_reached = true;
        self.macro_calls_skipped += skipped;
        self.macro_calls_skipped_by_limit += skipped;
    }
}

/// Wall-clock time spent in def-map finalization subphases.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DefMapFinalizationTimingStats {
    pub resolve_import_scopes: Duration,
    pub collect_expansion_attempts: Duration,
    pub apply_expansion_attempts: Duration,
    pub compile_macros: Duration,
    pub expand_macros: Duration,
    pub parse_generated_sources: Duration,
    pub collect_generated_items: Duration,
}

pub(super) struct DefMapFinalizationStatsSink<'a> {
    stats: Option<&'a mut DefMapFinalizationStats>,
}

impl<'a> DefMapFinalizationStatsSink<'a> {
    pub(super) fn new(stats: Option<&'a mut DefMapFinalizationStats>) -> Self {
        Self { stats }
    }

    pub(super) fn record(&mut self, f: impl FnOnce(&mut DefMapFinalizationStats)) {
        if let Some(stats) = self.stats.as_deref_mut() {
            f(stats);
        }
    }

    pub(super) fn start_timer(&self) -> Option<Instant> {
        self.stats.as_ref().map(|_| Instant::now())
    }

    pub(super) fn finish_timer(
        &mut self,
        timer: Option<Instant>,
        f: impl FnOnce(&mut DefMapFinalizationTimingStats, Duration),
    ) {
        let (Some(stats), Some(timer)) = (self.stats.as_deref_mut(), timer) else {
            return;
        };
        f(&mut stats.timings, timer.elapsed());
    }
}
