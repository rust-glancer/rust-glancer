use std::fmt;

use rg_project::DefMapFinalizationStats;
use serde::Serialize;

use super::stages::{duration_ms, format_duration_ms};

/// Counters and timings captured while def-map finalization resolves imports and macros.
#[derive(Debug, Serialize)]
pub(crate) struct DefMapFinalizationStatsReport {
    pub(crate) rounds: usize,
    pub(crate) expansion_passes: usize,
    pub(crate) expansion_pass_limit: usize,
    pub(crate) expansion_pass_limit_reached: bool,
    pub(crate) macro_calls_seen: usize,
    pub(crate) macro_calls_resolved: usize,
    pub(crate) macro_calls_unresolved: usize,
    pub(crate) macro_calls_skipped: usize,
    pub(crate) macro_calls_skipped_by_limit: usize,
    pub(crate) macro_calls_expanded: usize,
    pub(crate) macro_calls_failed: usize,
    pub(crate) macro_compile_failures: usize,
    pub(crate) macro_expand_failures: usize,
    pub(crate) macro_compile_attempts: usize,
    pub(crate) macro_compile_cache_hits: usize,
    pub(crate) macro_expand_attempts: usize,
    pub(crate) macro_expand_cache_hits: usize,
    pub(crate) generated_sources_parsed: usize,
    pub(crate) generated_source_parse_failures: usize,
    pub(crate) generated_items_seen: usize,
    pub(crate) top_failed_macros: Vec<MacroExpansionFailureMacroReport>,
    pub(crate) top_slow_macros: Vec<MacroExpansionSlowMacroReport>,
    pub(crate) top_unresolved_macros: Vec<MacroExpansionCountReport>,
    pub(crate) timings: DefMapFinalizationTimingStatsReport,
}

impl DefMapFinalizationStatsReport {
    pub(crate) fn capture(stats: &DefMapFinalizationStats) -> Self {
        Self {
            rounds: stats.rounds,
            expansion_passes: stats.expansion_passes,
            expansion_pass_limit: stats.expansion_pass_limit,
            expansion_pass_limit_reached: stats.expansion_pass_limit_reached,
            macro_calls_seen: stats.macro_calls_seen,
            macro_calls_resolved: stats.macro_calls_resolved,
            macro_calls_unresolved: stats.macro_calls_unresolved,
            macro_calls_skipped: stats.macro_calls_skipped,
            macro_calls_skipped_by_limit: stats.macro_calls_skipped_by_limit,
            macro_calls_expanded: stats.macro_calls_expanded,
            macro_calls_failed: stats.macro_calls_failed,
            macro_compile_failures: stats.macro_compile_failures,
            macro_expand_failures: stats.macro_expand_failures,
            macro_compile_attempts: stats.macro_compile_attempts,
            macro_compile_cache_hits: stats.macro_compile_cache_hits,
            macro_expand_attempts: stats.macro_expand_attempts,
            macro_expand_cache_hits: stats.macro_expand_cache_hits,
            generated_sources_parsed: stats.generated_sources_parsed,
            generated_source_parse_failures: stats.generated_source_parse_failures,
            generated_items_seen: stats.generated_items_seen,
            top_failed_macros: top_failed_macros(stats),
            top_slow_macros: top_slow_macros(stats),
            top_unresolved_macros: top_unresolved_macros(stats),
            timings: DefMapFinalizationTimingStatsReport {
                resolve_import_scopes_ms: duration_ms(stats.timings.resolve_import_scopes),
                collect_expansion_attempts_ms: duration_ms(
                    stats.timings.collect_expansion_attempts,
                ),
                apply_expansion_attempts_ms: duration_ms(stats.timings.apply_expansion_attempts),
                compile_macros_ms: duration_ms(stats.timings.compile_macros),
                expand_macros_ms: duration_ms(stats.timings.expand_macros),
                parse_generated_sources_ms: duration_ms(stats.timings.parse_generated_sources),
                collect_generated_items_ms: duration_ms(stats.timings.collect_generated_items),
            },
        }
    }
}

fn top_failed_macros(stats: &DefMapFinalizationStats) -> Vec<MacroExpansionFailureMacroReport> {
    let mut entries = stats
        .failed_macros
        .iter()
        .map(|(name, failure)| MacroExpansionFailureMacroReport {
            name: name.clone(),
            total: failure.total(),
            compile_failed: failure.compile_failed,
            expand_failed: failure.expand_failed,
            parse_failed: failure.parse_failed,
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        right
            .total
            .cmp(&left.total)
            .then_with(|| left.name.cmp(&right.name))
    });
    entries.truncate(20);
    entries
}

fn top_slow_macros(stats: &DefMapFinalizationStats) -> Vec<MacroExpansionSlowMacroReport> {
    let mut entries = stats
        .slow_macros
        .iter()
        .map(|(name, slow)| MacroExpansionSlowMacroReport {
            name: name.clone(),
            count: slow.count,
            total_ms: duration_ms(slow.total),
            average_ms: duration_ms(slow.average()),
            max_ms: duration_ms(slow.max),
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        right
            .total_ms
            .total_cmp(&left.total_ms)
            .then_with(|| left.name.cmp(&right.name))
    });
    entries.truncate(20);
    entries
}

fn top_unresolved_macros(stats: &DefMapFinalizationStats) -> Vec<MacroExpansionCountReport> {
    let mut entries = stats
        .unresolved_macros
        .iter()
        .map(|(name, count)| MacroExpansionCountReport {
            name: name.clone(),
            count: *count,
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.name.cmp(&right.name))
    });
    entries.truncate(20);
    entries
}

impl fmt::Display for DefMapFinalizationStatsReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "def-map finalization stats:")?;
        writeln!(f, "  rounds: {}", self.rounds)?;
        writeln!(f, "  expansion passes: {}", self.expansion_passes)?;
        if self.expansion_pass_limit_reached {
            writeln!(
                f,
                "  expansion pass limit reached: {} passes, {} pending calls skipped",
                self.expansion_pass_limit, self.macro_calls_skipped_by_limit,
            )?;
        }
        writeln!(
            f,
            "  macro calls: {} seen, {} resolved, {} expanded, {} failed, {} skipped, {} unresolved",
            self.macro_calls_seen,
            self.macro_calls_resolved,
            self.macro_calls_expanded,
            self.macro_calls_failed,
            self.macro_calls_skipped,
            self.macro_calls_unresolved,
        )?;
        writeln!(
            f,
            "  attempts: {} compile, {} expand",
            self.macro_compile_attempts, self.macro_expand_attempts,
        )?;
        writeln!(
            f,
            "  cache hits: {} compile, {} expand",
            self.macro_compile_cache_hits, self.macro_expand_cache_hits,
        )?;
        writeln!(
            f,
            "  failures: {} compile, {} expand, {} generated-source parse",
            self.macro_compile_failures,
            self.macro_expand_failures,
            self.generated_source_parse_failures,
        )?;
        writeln!(
            f,
            "  generated: {} sources parsed, {} syntax items seen",
            self.generated_sources_parsed, self.generated_items_seen,
        )?;
        if !self.top_failed_macros.is_empty() {
            writeln!(f, "  top failed macros:")?;
            writeln!(
                f,
                "    {:>8}  {:>8}  {:>8}  {:>8}  macro",
                "total", "compile", "expand", "parse",
            )?;
            for failed in &self.top_failed_macros {
                writeln!(
                    f,
                    "    {:>8}  {:>8}  {:>8}  {:>8}  {}",
                    failed.total,
                    failed.compile_failed,
                    failed.expand_failed,
                    failed.parse_failed,
                    failed.name,
                )?;
            }
        }
        if !self.top_slow_macros.is_empty() {
            writeln!(f, "  top slow macros:")?;
            writeln!(
                f,
                "    {:>12}  {:>10}  {:>10}  {:>8}  macro",
                "worker total", "avg", "max", "count",
            )?;
            for slow in &self.top_slow_macros {
                writeln!(
                    f,
                    "    {:>12}  {:>10}  {:>10}  {:>8}  {}",
                    format_duration_ms(slow.total_ms),
                    format_duration_ms(slow.average_ms),
                    format_duration_ms(slow.max_ms),
                    slow.count,
                    slow.name,
                )?;
            }
        }
        if !self.top_unresolved_macros.is_empty() {
            writeln!(f, "  top unresolved macros:")?;
            writeln!(f, "    {:>8}  macro", "seen")?;
            for unresolved in &self.top_unresolved_macros {
                writeln!(f, "    {:>8}  {}", unresolved.count, unresolved.name)?;
            }
        }
        writeln!(f, "  timings:")?;

        for (label, elapsed_ms) in [
            (
                "resolve import scopes",
                self.timings.resolve_import_scopes_ms,
            ),
            (
                "collect expansion attempts",
                self.timings.collect_expansion_attempts_ms,
            ),
            (
                "apply expansion attempts",
                self.timings.apply_expansion_attempts_ms,
            ),
            ("compile macros", self.timings.compile_macros_ms),
            ("expand macros", self.timings.expand_macros_ms),
            (
                "parse generated sources",
                self.timings.parse_generated_sources_ms,
            ),
            (
                "collect generated items",
                self.timings.collect_generated_items_ms,
            ),
        ] {
            writeln!(f, "    {:>10}  {label}", format_duration_ms(elapsed_ms))?;
        }

        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct MacroExpansionFailureMacroReport {
    pub(crate) name: String,
    pub(crate) total: usize,
    pub(crate) compile_failed: usize,
    pub(crate) expand_failed: usize,
    pub(crate) parse_failed: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct MacroExpansionSlowMacroReport {
    pub(crate) name: String,
    pub(crate) count: usize,
    pub(crate) total_ms: f64,
    pub(crate) average_ms: f64,
    pub(crate) max_ms: f64,
}

#[derive(Debug, Serialize)]
pub(crate) struct MacroExpansionCountReport {
    pub(crate) name: String,
    pub(crate) count: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct DefMapFinalizationTimingStatsReport {
    pub(crate) resolve_import_scopes_ms: f64,
    pub(crate) collect_expansion_attempts_ms: f64,
    pub(crate) apply_expansion_attempts_ms: f64,
    pub(crate) compile_macros_ms: f64,
    pub(crate) expand_macros_ms: f64,
    pub(crate) parse_generated_sources_ms: f64,
    pub(crate) collect_generated_items_ms: f64,
}
