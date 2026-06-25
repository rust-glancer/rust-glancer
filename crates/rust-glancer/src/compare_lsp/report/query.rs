//! Per-query comparison report sections.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::{
    compare_lsp::comparison::{
        HoverComparisonMetrics, MappedSetComparisonMetrics, NonComparableMetrics, QueryComparison,
        QueryComparisonResult, SetComparisonMetrics,
    },
    report::{
        ReportAlign, ReportDocumentBuilder, ReportRowBuilder, ReportTableBuilder, ReportUnit,
        ReportValue,
    },
};

use super::{duration_ms, optional_percent};

const HIGHLIGHT_LIMIT: usize = 10;

#[derive(Debug, Serialize)]
pub(super) struct QueryReport {
    label: String,
    method: String,
    rust_glancer_ms: f64,
    rust_analyzer_ms: f64,
    result: QueryResultReport,
}

impl QueryReport {
    pub(super) fn capture(query: &QueryComparison) -> Self {
        Self {
            label: query.label().to_string(),
            method: query.method().lsp_method().to_string(),
            rust_glancer_ms: duration_ms(query.rust_glancer_latency()),
            rust_analyzer_ms: duration_ms(query.rust_analyzer_latency()),
            result: QueryResultReport::capture(query.result()),
        }
    }

    pub(super) fn append_sections(
        document: ReportDocumentBuilder,
        queries: &[Self],
    ) -> ReportDocumentBuilder {
        let document = Self::append_highlight_section(document, queries);
        let document = Self::append_query_section(document, queries);

        Self::append_failures_section(document, queries)
    }

    fn append_highlight_section(
        document: ReportDocumentBuilder,
        queries: &[Self],
    ) -> ReportDocumentBuilder {
        if queries.is_empty() {
            return document;
        }

        document.section("query_highlights", |section| {
            section.group("comparison", "Comparison");
            section.table("slowest_queries", |table| {
                Self::configure_slowest_table(table);
                for query in Self::slowest_queries(queries) {
                    query.append_slowest_row(table);
                }
            });

            let lowest_recall = Self::lowest_recall_queries(queries);
            if !lowest_recall.is_empty() {
                section.table("lowest_recall", |table| {
                    Self::configure_recall_table(table);
                    for (query, counts) in lowest_recall {
                        query.append_recall_row(table, counts);
                    }
                });
            }

            let lowest_precision = Self::lowest_precision_queries(queries);
            if !lowest_precision.is_empty() {
                section.table("lowest_precision", |table| {
                    Self::configure_precision_table(table);
                    for (query, counts) in lowest_precision {
                        query.append_precision_row(table, counts);
                    }
                });
            }

            let hover_gaps = Self::hover_gap_queries(queries);
            if !hover_gaps.is_empty() {
                section.table("hover_gaps", |table| {
                    Self::configure_hover_gap_table(table);
                    for query in hover_gaps {
                        query.append_hover_gap_row(table);
                    }
                });
            }
        })
    }

    fn append_query_section(
        document: ReportDocumentBuilder,
        queries: &[Self],
    ) -> ReportDocumentBuilder {
        document.section("queries", |section| {
            section.group("comparison", "Comparison");
            for method in Self::query_methods(queries) {
                section.table(Self::table_key_for_method(method), |table| {
                    table.title(method);
                    Self::configure_query_table(table);
                    for query in queries.iter().filter(|query| query.method == method) {
                        query.append_query_row(table);
                    }
                });
            }
        })
    }

    fn query_methods(queries: &[Self]) -> Vec<&str> {
        let mut methods = Vec::new();
        let mut seen = BTreeSet::new();
        for query in queries {
            if seen.insert(query.method.as_str()) {
                methods.push(query.method.as_str());
            }
        }
        methods
    }

    fn table_key_for_method(method: &str) -> String {
        let mut key = String::from("queries_");
        for character in method.chars() {
            if character.is_ascii_alphanumeric() {
                key.push(character.to_ascii_lowercase());
            } else {
                key.push('_');
            }
        }
        key
    }

    fn slowest_queries(queries: &[Self]) -> Vec<&Self> {
        let mut slowest = queries.iter().collect::<Vec<_>>();
        slowest.sort_by(|left, right| right.rust_glancer_ms.total_cmp(&left.rust_glancer_ms));
        slowest.truncate(HIGHLIGHT_LIMIT);
        slowest
    }

    fn lowest_recall_queries(queries: &[Self]) -> Vec<(&Self, QueryCounts)> {
        let mut lowest = queries
            .iter()
            .filter_map(|query| query.counts().map(|counts| (query, counts)))
            .filter(|(_, counts)| counts.recall_percent.is_some_and(|recall| recall < 100.0))
            .collect::<Vec<_>>();
        lowest.sort_by(|(_, left), (_, right)| {
            left.recall_percent
                .unwrap_or(100.0)
                .total_cmp(&right.recall_percent.unwrap_or(100.0))
        });
        lowest.truncate(HIGHLIGHT_LIMIT);
        lowest
    }

    fn lowest_precision_queries(queries: &[Self]) -> Vec<(&Self, QueryCounts)> {
        let mut lowest = queries
            .iter()
            .filter_map(|query| query.counts().map(|counts| (query, counts)))
            .filter(|(_, counts)| {
                counts
                    .precision_percent
                    .is_some_and(|precision| precision < 100.0)
            })
            .collect::<Vec<_>>();
        lowest.sort_by(|(_, left), (_, right)| {
            left.precision_percent
                .unwrap_or(100.0)
                .total_cmp(&right.precision_percent.unwrap_or(100.0))
        });
        lowest.truncate(HIGHLIGHT_LIMIT);
        lowest
    }

    fn hover_gap_queries(queries: &[Self]) -> Vec<&Self> {
        queries
            .iter()
            .filter(|query| query.hover().is_some_and(|hover| !hover.agreement))
            .take(HIGHLIGHT_LIMIT)
            .collect()
    }

    fn counts(&self) -> Option<QueryCounts> {
        self.result.counts()
    }

    fn hover(&self) -> Option<&HoverQueryReport> {
        self.result.hover()
    }

    fn configure_slowest_table(table: &mut ReportTableBuilder) {
        table
            .text_column("method")
            .text_column("query")
            .duration_column_as("rust_glancer_ms", "rust-glancer")
            .duration_column_as("rust_analyzer_ms", "rust-analyzer")
            .text_column("outcome")
            .column_as(
                "recall",
                "Recall",
                ReportAlign::Right,
                Some(ReportUnit::Percent),
            )
            .column_as(
                "precision",
                "Precision",
                ReportAlign::Right,
                Some(ReportUnit::Percent),
            );
    }

    fn configure_recall_table(table: &mut ReportTableBuilder) {
        table
            .text_column("method")
            .text_column("query")
            .column_as(
                "recall",
                "Recall",
                ReportAlign::Right,
                Some(ReportUnit::Percent),
            )
            .count_column("rust_glancer_count")
            .count_column("rust_analyzer_count")
            .count_column("matched")
            .count_column("missing");
    }

    fn configure_precision_table(table: &mut ReportTableBuilder) {
        table
            .text_column("method")
            .text_column("query")
            .column_as(
                "precision",
                "Precision",
                ReportAlign::Right,
                Some(ReportUnit::Percent),
            )
            .count_column("rust_glancer_count")
            .count_column("rust_analyzer_count")
            .count_column("matched")
            .count_column("extra");
    }

    fn configure_hover_gap_table(table: &mut ReportTableBuilder) {
        table
            .text_column("method")
            .text_column("query")
            .duration_column_as("rust_glancer_ms", "rust-glancer")
            .duration_column_as("rust_analyzer_ms", "rust-analyzer")
            .column_as(
                "rust_glancer_present",
                "rust-glancer present",
                ReportAlign::Center,
                None,
            )
            .column_as(
                "rust_analyzer_present",
                "rust-analyzer present",
                ReportAlign::Center,
                None,
            );
    }

    fn append_slowest_row(&self, table: &mut ReportTableBuilder) {
        table.row(|row| {
            row.text("method", &self.method)
                .text("query", &self.label)
                .duration_ms("rust_glancer_ms", self.rust_glancer_ms)
                .duration_ms("rust_analyzer_ms", self.rust_analyzer_ms)
                .text("outcome", self.result.kind());

            if let Some(counts) = self.counts() {
                counts.append_score_cells(row);
            }
        });
    }

    fn append_recall_row(&self, table: &mut ReportTableBuilder, counts: QueryCounts) {
        table.row(|row| {
            row.text("method", &self.method).text("query", &self.label);
            counts.append_recall_cells(row);
        });
    }

    fn append_precision_row(&self, table: &mut ReportTableBuilder, counts: QueryCounts) {
        table.row(|row| {
            row.text("method", &self.method).text("query", &self.label);
            counts.append_precision_cells(row);
        });
    }

    fn append_hover_gap_row(&self, table: &mut ReportTableBuilder) {
        let Some(hover) = self.hover() else {
            return;
        };

        table.row(|row| {
            row.text("method", &self.method)
                .text("query", &self.label)
                .duration_ms("rust_glancer_ms", self.rust_glancer_ms)
                .duration_ms("rust_analyzer_ms", self.rust_analyzer_ms)
                .value(
                    "rust_glancer_present",
                    ReportValue::Bool(hover.rust_glancer_present),
                )
                .value(
                    "rust_analyzer_present",
                    ReportValue::Bool(hover.rust_analyzer_present),
                );
        });
    }

    fn configure_query_table(table: &mut ReportTableBuilder) {
        table
            .text_column("query")
            .duration_column_as("rust_glancer_ms", "rust-glancer")
            .duration_column_as("rust_analyzer_ms", "rust-analyzer")
            .text_column("outcome")
            .count_column("rust_glancer_count")
            .count_column("rust_analyzer_count")
            .count_column("matched")
            .count_column("missing")
            .count_column("extra")
            .column_as(
                "recall",
                "Recall",
                ReportAlign::Right,
                Some(ReportUnit::Percent),
            )
            .column_as(
                "precision",
                "Precision",
                ReportAlign::Right,
                Some(ReportUnit::Percent),
            );
    }

    fn append_query_row(&self, table: &mut ReportTableBuilder) {
        table.row(|row| {
            row.text("query", &self.label)
                .duration_ms("rust_glancer_ms", self.rust_glancer_ms)
                .duration_ms("rust_analyzer_ms", self.rust_analyzer_ms)
                .text("outcome", self.result.kind());

            self.result.append_query_cells(row);
        });
    }

    fn append_failures_section(
        document: ReportDocumentBuilder,
        queries: &[Self],
    ) -> ReportDocumentBuilder {
        let failures = queries
            .iter()
            .filter(|query| query.is_non_comparable())
            .collect::<Vec<_>>();

        if failures.is_empty() {
            return document;
        }

        document.section("failures", |section| {
            section.group("comparison", "Comparison");
            section.table("non_comparable_queries", |table| {
                Self::configure_failure_table(table);
                for query in failures {
                    query.append_failure_row(table);
                }
            });
        })
    }

    fn configure_failure_table(table: &mut ReportTableBuilder) {
        table
            .text_column("method")
            .text_column("query")
            .text_column("rust_glancer")
            .text_column("rust_glancer_detail")
            .text_column("rust_analyzer")
            .text_column("rust_analyzer_detail");
    }

    fn append_failure_row(&self, table: &mut ReportTableBuilder) {
        let QueryResultReport::NonComparable(non_comparable) = &self.result else {
            return;
        };

        table.row(|row| {
            row.text("method", &self.method)
                .text("query", &self.label)
                .text("rust_glancer", &non_comparable.rust_glancer_status)
                .text(
                    "rust_glancer_detail",
                    non_comparable.rust_glancer_detail.as_deref().unwrap_or(""),
                )
                .text("rust_analyzer", &non_comparable.rust_analyzer_status)
                .text(
                    "rust_analyzer_detail",
                    non_comparable.rust_analyzer_detail.as_deref().unwrap_or(""),
                );
        });
    }

    fn is_non_comparable(&self) -> bool {
        matches!(self.result, QueryResultReport::NonComparable(_))
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum QueryResultReport {
    Locations(LocationQueryReport),
    PrepareRenames(RangeQueryReport),
    RenameEdits(LocationQueryReport),
    Ranges(RangeQueryReport),
    Symbols(SymbolQueryReport),
    InlayHints(RangeQueryReport),
    Hover(HoverQueryReport),
    NonComparable(NonComparableQueryReport),
}

impl QueryResultReport {
    fn capture(result: &QueryComparisonResult) -> Self {
        match result {
            QueryComparisonResult::Locations(locations) => {
                Self::Locations(locations.metrics().into())
            }
            QueryComparisonResult::PrepareRenames(rename) => {
                Self::PrepareRenames(rename.metrics().into())
            }
            QueryComparisonResult::RenameEdits(rename) => {
                Self::RenameEdits(rename.metrics().into())
            }
            QueryComparisonResult::Ranges(ranges) => Self::Ranges(ranges.metrics().into()),
            QueryComparisonResult::Symbols(symbols) => Self::Symbols(symbols.metrics().into()),
            QueryComparisonResult::InlayHints(hints) => Self::InlayHints(hints.metrics().into()),
            QueryComparisonResult::Hover(hover) => Self::Hover(hover.metrics().into()),
            QueryComparisonResult::NonComparable(non_comparable) => {
                Self::NonComparable(non_comparable.metrics().into())
            }
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Locations(_) => "locations",
            Self::PrepareRenames(_) => "prepare_renames",
            Self::RenameEdits(_) => "rename_edits",
            Self::Ranges(_) => "ranges",
            Self::Symbols(_) => "symbols",
            Self::InlayHints(_) => "inlay_hints",
            Self::Hover(_) => "hover",
            Self::NonComparable(_) => "non_comparable",
        }
    }

    fn append_query_cells(&self, row: &mut ReportRowBuilder) {
        match self {
            Self::Locations(locations) => locations.append_query_cells(row),
            Self::PrepareRenames(rename) => rename.append_query_cells(row),
            Self::RenameEdits(rename) => rename.append_query_cells(row),
            Self::Ranges(ranges) => ranges.append_query_cells(row),
            Self::Symbols(symbols) => symbols.append_query_cells(row),
            Self::InlayHints(hints) => hints.append_query_cells(row),
            Self::Hover(_) | Self::NonComparable(_) => {}
        }
    }

    fn counts(&self) -> Option<QueryCounts> {
        match self {
            Self::Locations(locations) => Some(locations.into()),
            Self::PrepareRenames(rename) => Some(rename.into()),
            Self::RenameEdits(rename) => Some(rename.into()),
            Self::Ranges(ranges) => Some(ranges.into()),
            Self::Symbols(symbols) => Some(symbols.into()),
            Self::InlayHints(hints) => Some(hints.into()),
            Self::Hover(_) | Self::NonComparable(_) => None,
        }
    }

    fn hover(&self) -> Option<&HoverQueryReport> {
        match self {
            Self::Hover(hover) => Some(hover),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct QueryCounts {
    rust_glancer_count: usize,
    rust_analyzer_count: usize,
    matched_count: usize,
    missing_count: usize,
    extra_count: usize,
    recall_percent: Option<f64>,
    precision_percent: Option<f64>,
}

impl QueryCounts {
    fn append_query_cells(self, row: &mut ReportRowBuilder) {
        row.value(
            "rust_glancer_count",
            ReportValue::count(self.rust_glancer_count),
        )
        .value(
            "rust_analyzer_count",
            ReportValue::count(self.rust_analyzer_count),
        )
        .value("matched", ReportValue::count(self.matched_count))
        .value("missing", ReportValue::count(self.missing_count))
        .value("extra", ReportValue::count(self.extra_count))
        .value("recall", optional_percent(self.recall_percent))
        .value("precision", optional_percent(self.precision_percent));
    }

    fn append_score_cells(self, row: &mut ReportRowBuilder) {
        row.value("recall", optional_percent(self.recall_percent))
            .value("precision", optional_percent(self.precision_percent));
    }

    fn append_recall_cells(self, row: &mut ReportRowBuilder) {
        row.value("recall", optional_percent(self.recall_percent))
            .value(
                "rust_glancer_count",
                ReportValue::count(self.rust_glancer_count),
            )
            .value(
                "rust_analyzer_count",
                ReportValue::count(self.rust_analyzer_count),
            )
            .value("matched", ReportValue::count(self.matched_count))
            .value("missing", ReportValue::count(self.missing_count));
    }

    fn append_precision_cells(self, row: &mut ReportRowBuilder) {
        row.value("precision", optional_percent(self.precision_percent))
            .value(
                "rust_glancer_count",
                ReportValue::count(self.rust_glancer_count),
            )
            .value(
                "rust_analyzer_count",
                ReportValue::count(self.rust_analyzer_count),
            )
            .value("matched", ReportValue::count(self.matched_count))
            .value("extra", ReportValue::count(self.extra_count));
    }
}

#[derive(Debug, Serialize)]
struct LocationQueryReport {
    rust_glancer_count: usize,
    rust_analyzer_count: usize,
    matched_count: usize,
    missing_count: usize,
    extra_count: usize,
    rust_glancer_unmapped_count: usize,
    rust_analyzer_unmapped_count: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    rust_glancer_unmapped: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    rust_analyzer_unmapped: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recall_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    precision_percent: Option<f64>,
}

impl LocationQueryReport {
    fn append_query_cells(&self, row: &mut ReportRowBuilder) {
        QueryCounts::from(self).append_query_cells(row);
    }
}

impl From<&LocationQueryReport> for QueryCounts {
    fn from(report: &LocationQueryReport) -> Self {
        Self {
            rust_glancer_count: report.rust_glancer_count,
            rust_analyzer_count: report.rust_analyzer_count,
            matched_count: report.matched_count,
            missing_count: report.missing_count,
            extra_count: report.extra_count,
            recall_percent: report.recall_percent,
            precision_percent: report.precision_percent,
        }
    }
}

impl From<MappedSetComparisonMetrics> for LocationQueryReport {
    fn from(metrics: MappedSetComparisonMetrics) -> Self {
        Self {
            rust_glancer_count: metrics.set.rust_glancer_count,
            rust_analyzer_count: metrics.set.rust_analyzer_count,
            matched_count: metrics.set.matched_count,
            missing_count: metrics.set.missing_count,
            extra_count: metrics.set.extra_count,
            rust_glancer_unmapped_count: metrics.rust_glancer_unmapped_count,
            rust_analyzer_unmapped_count: metrics.rust_analyzer_unmapped_count,
            rust_glancer_unmapped: metrics.rust_glancer_unmapped,
            rust_analyzer_unmapped: metrics.rust_analyzer_unmapped,
            recall_percent: metrics.set.recall_percent,
            precision_percent: metrics.set.precision_percent,
        }
    }
}

#[derive(Debug, Serialize)]
struct RangeQueryReport {
    rust_glancer_count: usize,
    rust_analyzer_count: usize,
    matched_count: usize,
    missing_count: usize,
    extra_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    recall_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    precision_percent: Option<f64>,
}

impl RangeQueryReport {
    fn append_query_cells(&self, row: &mut ReportRowBuilder) {
        QueryCounts::from(self).append_query_cells(row);
    }
}

impl From<&RangeQueryReport> for QueryCounts {
    fn from(report: &RangeQueryReport) -> Self {
        Self {
            rust_glancer_count: report.rust_glancer_count,
            rust_analyzer_count: report.rust_analyzer_count,
            matched_count: report.matched_count,
            missing_count: report.missing_count,
            extra_count: report.extra_count,
            recall_percent: report.recall_percent,
            precision_percent: report.precision_percent,
        }
    }
}

impl From<SetComparisonMetrics> for RangeQueryReport {
    fn from(metrics: SetComparisonMetrics) -> Self {
        Self {
            rust_glancer_count: metrics.rust_glancer_count,
            rust_analyzer_count: metrics.rust_analyzer_count,
            matched_count: metrics.matched_count,
            missing_count: metrics.missing_count,
            extra_count: metrics.extra_count,
            recall_percent: metrics.recall_percent,
            precision_percent: metrics.precision_percent,
        }
    }
}

#[derive(Debug, Serialize)]
struct SymbolQueryReport {
    rust_glancer_count: usize,
    rust_analyzer_count: usize,
    matched_count: usize,
    missing_count: usize,
    extra_count: usize,
    rust_glancer_unmapped_count: usize,
    rust_analyzer_unmapped_count: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    rust_glancer_unmapped: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    rust_analyzer_unmapped: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recall_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    precision_percent: Option<f64>,
}

impl SymbolQueryReport {
    fn append_query_cells(&self, row: &mut ReportRowBuilder) {
        QueryCounts::from(self).append_query_cells(row);
    }
}

impl From<&SymbolQueryReport> for QueryCounts {
    fn from(report: &SymbolQueryReport) -> Self {
        Self {
            rust_glancer_count: report.rust_glancer_count,
            rust_analyzer_count: report.rust_analyzer_count,
            matched_count: report.matched_count,
            missing_count: report.missing_count,
            extra_count: report.extra_count,
            recall_percent: report.recall_percent,
            precision_percent: report.precision_percent,
        }
    }
}

impl From<MappedSetComparisonMetrics> for SymbolQueryReport {
    fn from(metrics: MappedSetComparisonMetrics) -> Self {
        Self {
            rust_glancer_count: metrics.set.rust_glancer_count,
            rust_analyzer_count: metrics.set.rust_analyzer_count,
            matched_count: metrics.set.matched_count,
            missing_count: metrics.set.missing_count,
            extra_count: metrics.set.extra_count,
            rust_glancer_unmapped_count: metrics.rust_glancer_unmapped_count,
            rust_analyzer_unmapped_count: metrics.rust_analyzer_unmapped_count,
            rust_glancer_unmapped: metrics.rust_glancer_unmapped,
            rust_analyzer_unmapped: metrics.rust_analyzer_unmapped,
            recall_percent: metrics.set.recall_percent,
            precision_percent: metrics.set.precision_percent,
        }
    }
}

#[derive(Debug, Serialize)]
struct HoverQueryReport {
    rust_glancer_present: bool,
    rust_analyzer_present: bool,
    agreement: bool,
    rust_glancer_missing: bool,
    rust_glancer_extra_present: bool,
}

impl From<HoverComparisonMetrics> for HoverQueryReport {
    fn from(metrics: HoverComparisonMetrics) -> Self {
        Self {
            rust_glancer_present: metrics.rust_glancer_present,
            rust_analyzer_present: metrics.rust_analyzer_present,
            agreement: metrics.agreement,
            rust_glancer_missing: metrics.rust_glancer_missing,
            rust_glancer_extra_present: metrics.rust_glancer_extra_present,
        }
    }
}

#[derive(Debug, Serialize)]
struct NonComparableQueryReport {
    rust_glancer_status: String,
    rust_analyzer_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    rust_glancer_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rust_analyzer_detail: Option<String>,
}

impl From<NonComparableMetrics> for NonComparableQueryReport {
    fn from(metrics: NonComparableMetrics) -> Self {
        Self {
            rust_glancer_status: metrics.rust_glancer_status.label().to_string(),
            rust_analyzer_status: metrics.rust_analyzer_status.label().to_string(),
            rust_glancer_detail: metrics.rust_glancer_detail,
            rust_analyzer_detail: metrics.rust_analyzer_detail,
        }
    }
}
