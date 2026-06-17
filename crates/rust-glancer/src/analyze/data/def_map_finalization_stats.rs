use rg_project::DefMapFinalizationStats;
use serde::Serialize;

use super::stages::duration_ms;
use crate::analyze::report::{ReportFieldsBuilder, ReportSectionBuilder, ReportTableBuilder};

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

    pub(super) fn append_document(&self, section: &mut ReportSectionBuilder) {
        section.title("def-map finalization stats");
        section.fields("summary", |fields| {
            fields
                .count("rounds", self.rounds)
                .count("expansion_passes", self.expansion_passes)
                .count("expansion_pass_limit", self.expansion_pass_limit)
                .bool(
                    "expansion_pass_limit_reached",
                    self.expansion_pass_limit_reached,
                );
        });
        section.warning_if(
            self.expansion_pass_limit_reached,
            format!(
                "{} passes reached, {} pending calls skipped",
                self.expansion_pass_limit, self.macro_calls_skipped_by_limit,
            ),
        );
        section.fields("macro_calls", |fields| {
            self.append_macro_call_fields(fields)
        });
        section.fields("attempts", |fields| {
            fields
                .count_as("compile", "compile", self.macro_compile_attempts)
                .count_as("expand", "expand", self.macro_expand_attempts);
        });
        section.fields("cache_hits", |fields| {
            fields
                .count_as("compile", "compile", self.macro_compile_cache_hits)
                .count_as("expand", "expand", self.macro_expand_cache_hits);
        });
        section.fields("failures", |fields| {
            fields
                .count_as("compile", "compile", self.macro_compile_failures)
                .count_as("expand", "expand", self.macro_expand_failures)
                .count_as(
                    "generated_source_parse",
                    "generated-source parse",
                    self.generated_source_parse_failures,
                );
        });
        section.fields("generated", |fields| {
            fields
                .count_as(
                    "sources_parsed",
                    "sources parsed",
                    self.generated_sources_parsed,
                )
                .count_as("items_seen", "syntax items seen", self.generated_items_seen);
        });
        section.table_if(
            !self.top_failed_macros.is_empty(),
            "top_failed_macros",
            |table| {
                MacroExpansionFailureMacroReport::append_table(table, &self.top_failed_macros);
            },
        );
        section.table_if(
            !self.top_slow_macros.is_empty(),
            "top_slow_macros",
            |table| {
                MacroExpansionSlowMacroReport::append_table(table, &self.top_slow_macros);
            },
        );
        section.table_if(
            !self.top_unresolved_macros.is_empty(),
            "top_unresolved_macros",
            |table| {
                MacroExpansionCountReport::append_table(table, &self.top_unresolved_macros);
            },
        );
        section.table("timings", |table| self.timings.append_table(table));
    }

    fn append_macro_call_fields(&self, fields: &mut ReportFieldsBuilder) {
        fields
            .count_as("seen", "seen", self.macro_calls_seen)
            .count_as("resolved", "resolved", self.macro_calls_resolved)
            .count_as("expanded", "expanded", self.macro_calls_expanded)
            .count_as("failed", "failed", self.macro_calls_failed)
            .count_as("skipped", "skipped", self.macro_calls_skipped)
            .count_as("unresolved", "unresolved", self.macro_calls_unresolved)
            .count_as(
                "skipped_by_limit",
                "skipped by limit",
                self.macro_calls_skipped_by_limit,
            );
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

#[derive(Debug, Serialize)]
pub(crate) struct MacroExpansionFailureMacroReport {
    pub(crate) name: String,
    pub(crate) total: usize,
    pub(crate) compile_failed: usize,
    pub(crate) expand_failed: usize,
    pub(crate) parse_failed: usize,
}

impl MacroExpansionFailureMacroReport {
    fn append_table(table: &mut ReportTableBuilder, rows: &[Self]) {
        table
            .title("top failed macros")
            .count_column("total")
            .count_column_as("compile_failed", "compile")
            .count_column_as("expand_failed", "expand")
            .count_column_as("parse_failed", "parse")
            .text_column_as("name", "macro");

        for failed in rows {
            table.row(|row| {
                row.count("total", failed.total)
                    .count("compile_failed", failed.compile_failed)
                    .count("expand_failed", failed.expand_failed)
                    .count("parse_failed", failed.parse_failed)
                    .text("name", &failed.name);
            });
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct MacroExpansionSlowMacroReport {
    pub(crate) name: String,
    pub(crate) count: usize,
    pub(crate) total_ms: f64,
    pub(crate) average_ms: f64,
    pub(crate) max_ms: f64,
}

impl MacroExpansionSlowMacroReport {
    fn append_table(table: &mut ReportTableBuilder, rows: &[Self]) {
        table
            .title("top slow macros")
            .duration_column_as("total_ms", "worker total")
            .duration_column_as("average_ms", "avg")
            .duration_column_as("max_ms", "max")
            .count_column("count")
            .text_column_as("name", "macro");

        for slow in rows {
            table.row(|row| {
                row.duration_ms("total_ms", slow.total_ms)
                    .duration_ms("average_ms", slow.average_ms)
                    .duration_ms("max_ms", slow.max_ms)
                    .count("count", slow.count)
                    .text("name", &slow.name);
            });
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct MacroExpansionCountReport {
    pub(crate) name: String,
    pub(crate) count: usize,
}

impl MacroExpansionCountReport {
    fn append_table(table: &mut ReportTableBuilder, rows: &[Self]) {
        table
            .title("top unresolved macros")
            .count_column_as("count", "seen")
            .text_column_as("name", "macro");

        for unresolved in rows {
            table.row(|row| {
                row.count("count", unresolved.count)
                    .text("name", &unresolved.name);
            });
        }
    }
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

impl DefMapFinalizationTimingStatsReport {
    fn append_table(&self, table: &mut ReportTableBuilder) {
        table.duration_column("elapsed").text_column("step");

        for (step, elapsed_ms) in [
            ("resolve import scopes", self.resolve_import_scopes_ms),
            (
                "collect expansion attempts",
                self.collect_expansion_attempts_ms,
            ),
            ("apply expansion attempts", self.apply_expansion_attempts_ms),
            ("compile macros", self.compile_macros_ms),
            ("expand macros", self.expand_macros_ms),
            ("parse generated sources", self.parse_generated_sources_ms),
            ("collect generated items", self.collect_generated_items_ms),
        ] {
            table.row(|row| {
                row.duration_ms("elapsed", elapsed_ms).text("step", step);
            });
        }
    }
}
