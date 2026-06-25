//! Per-query comparison report sections.

use serde::Serialize;

use crate::{
    compare_lsp::comparison::{QueryComparison, QueryComparisonResult},
    report::{
        ReportAlign, ReportDocumentBuilder, ReportRowBuilder, ReportTableBuilder, ReportUnit,
        ReportValue,
    },
};

use super::{duration_ms, optional_percent};

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
        let document = Self::append_query_section(document, queries);

        Self::append_failures_section(document, queries)
    }

    fn append_query_section(
        document: ReportDocumentBuilder,
        queries: &[Self],
    ) -> ReportDocumentBuilder {
        document.section("queries", |section| {
            section.group("comparison", "Comparison");
            section.table("queries", |table| {
                Self::configure_query_table(table);
                for query in queries {
                    query.append_query_row(table);
                }
            });
        })
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

    fn configure_query_table(table: &mut ReportTableBuilder) {
        table
            .text_column("method")
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

    fn configure_failure_table(table: &mut ReportTableBuilder) {
        table
            .text_column("method")
            .text_column("query")
            .text_column("rust_glancer")
            .text_column("rust_glancer_detail")
            .text_column("rust_analyzer")
            .text_column("rust_analyzer_detail");
    }

    fn append_query_row(&self, table: &mut ReportTableBuilder) {
        table.row(|row| {
            row.text("method", &self.method)
                .text("query", &self.label)
                .duration_ms("rust_glancer_ms", self.rust_glancer_ms)
                .duration_ms("rust_analyzer_ms", self.rust_analyzer_ms)
                .text("outcome", self.result.kind());

            self.result.append_query_cells(row);
        });
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
    Hover(HoverQueryReport),
    NonComparable(NonComparableQueryReport),
}

impl QueryResultReport {
    fn capture(result: &QueryComparisonResult) -> Self {
        match result {
            QueryComparisonResult::Locations(locations) => Self::Locations(LocationQueryReport {
                rust_glancer_count: locations.rust_glancer_count(),
                rust_analyzer_count: locations.rust_analyzer_count(),
                matched_count: locations.matched_count(),
                missing_count: locations.missing_count(),
                extra_count: locations.extra_count(),
                rust_glancer_unmapped_count: locations.rust_glancer_unmapped_count(),
                rust_analyzer_unmapped_count: locations.rust_analyzer_unmapped_count(),
                rust_glancer_unmapped: locations.rust_glancer_unmapped().to_vec(),
                rust_analyzer_unmapped: locations.rust_analyzer_unmapped().to_vec(),
                recall_percent: locations.completeness_percent(),
                precision_percent: locations.precision_signal_percent(),
            }),
            QueryComparisonResult::Hover(hover) => Self::Hover(HoverQueryReport {
                rust_glancer_present: hover.rust_glancer_present(),
                rust_analyzer_present: hover.rust_analyzer_present(),
                agreement: hover.agrees(),
                rust_glancer_missing: hover.rust_glancer_missing(),
                rust_glancer_extra_present: hover.rust_glancer_extra_present(),
            }),
            QueryComparisonResult::NonComparable(non_comparable) => {
                Self::NonComparable(NonComparableQueryReport {
                    rust_glancer_status: non_comparable.rust_glancer_status().label().to_string(),
                    rust_analyzer_status: non_comparable.rust_analyzer_status().label().to_string(),
                    rust_glancer_detail: non_comparable.rust_glancer_detail().map(str::to_string),
                    rust_analyzer_detail: non_comparable.rust_analyzer_detail().map(str::to_string),
                })
            }
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Locations(_) => "locations",
            Self::Hover(_) => "hover",
            Self::NonComparable(_) => "non_comparable",
        }
    }

    fn append_query_cells(&self, row: &mut ReportRowBuilder) {
        if let Self::Locations(locations) = self {
            locations.append_query_cells(row);
        }
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
}

#[derive(Debug, Serialize)]
struct HoverQueryReport {
    rust_glancer_present: bool,
    rust_analyzer_present: bool,
    agreement: bool,
    rust_glancer_missing: bool,
    rust_glancer_extra_present: bool,
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
