//! Aggregate comparison report section.

use serde::Serialize;

use crate::{
    compare_lsp::comparison::{MethodAggregate, MethodAggregateData},
    report::{
        ReportAlign, ReportDocumentBuilder, ReportRowBuilder, ReportTableBuilder, ReportUnit,
        ReportValue,
    },
};

use super::optional_percent;

#[derive(Debug, Serialize)]
pub(super) struct MethodAggregateReport {
    method: String,
    query_count: usize,
    comparable_count: usize,
    non_comparable_count: usize,
    #[serde(flatten)]
    data: MethodAggregateDataReport,
}

impl MethodAggregateReport {
    pub(super) fn capture(aggregate: &MethodAggregate) -> Self {
        match aggregate.data() {
            MethodAggregateData::Locations(locations) => Self {
                method: aggregate.method().lsp_method().to_string(),
                query_count: locations.query_count(),
                comparable_count: locations.comparable_count(),
                non_comparable_count: locations.non_comparable_count(),
                data: MethodAggregateDataReport::Locations(LocationAggregateReport {
                    rust_glancer_count: locations.rust_glancer_locations(),
                    rust_analyzer_count: locations.rust_analyzer_locations(),
                    matched_count: locations.matched_locations(),
                    missing_count: locations.missing_locations(),
                    extra_count: locations.extra_locations(),
                    rust_glancer_unmapped_count: locations.rust_glancer_unmapped_locations(),
                    rust_analyzer_unmapped_count: locations.rust_analyzer_unmapped_locations(),
                    recall_percent: locations.weighted_completeness_percent(),
                    precision_percent: locations.precision_signal_percent(),
                }),
            },
            MethodAggregateData::Hover(hover) => Self {
                method: aggregate.method().lsp_method().to_string(),
                query_count: hover.query_count(),
                comparable_count: hover.comparable_count(),
                non_comparable_count: hover.non_comparable_count(),
                data: MethodAggregateDataReport::Hover(HoverAggregateReport {
                    agreement_count: hover.agreement_count(),
                    rust_glancer_missing_count: hover.rust_glancer_missing_count(),
                    rust_glancer_extra_present_count: hover.rust_glancer_extra_present_count(),
                }),
            },
        }
    }

    pub(super) fn append_section(
        document: ReportDocumentBuilder,
        aggregates: &[Self],
    ) -> ReportDocumentBuilder {
        document.section("aggregates", |section| {
            section.group("comparison", "Comparison");
            section.table("aggregates", |table| {
                Self::configure_table(table);
                for aggregate in aggregates {
                    aggregate.append_row(table);
                }
            });
        })
    }

    fn configure_table(table: &mut ReportTableBuilder) {
        table
            .text_column("method")
            .count_column("queries")
            .count_column("comparable")
            .count_column("non_comparable")
            .count_column("rust_glancer")
            .count_column("rust_analyzer")
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
            )
            .count_column("hover_agreements")
            .count_column("hover_missing")
            .count_column("hover_extra_present")
            .count_column("unmapped_rg")
            .count_column("unmapped_ra");
    }

    fn append_row(&self, table: &mut ReportTableBuilder) {
        table.row(|row| {
            row.text("method", &self.method)
                .value("queries", ReportValue::count(self.query_count))
                .value("comparable", ReportValue::count(self.comparable_count))
                .value(
                    "non_comparable",
                    ReportValue::count(self.non_comparable_count),
                );

            self.data.append_row_cells(row);
        });
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum MethodAggregateDataReport {
    Locations(LocationAggregateReport),
    Hover(HoverAggregateReport),
}

impl MethodAggregateDataReport {
    fn append_row_cells(&self, row: &mut ReportRowBuilder) {
        match self {
            Self::Locations(locations) => locations.append_row_cells(row),
            Self::Hover(hover) => hover.append_row_cells(row),
        }
    }
}

#[derive(Debug, Serialize)]
struct LocationAggregateReport {
    rust_glancer_count: usize,
    rust_analyzer_count: usize,
    matched_count: usize,
    missing_count: usize,
    extra_count: usize,
    rust_glancer_unmapped_count: usize,
    rust_analyzer_unmapped_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    recall_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    precision_percent: Option<f64>,
}

impl LocationAggregateReport {
    fn append_row_cells(&self, row: &mut ReportRowBuilder) {
        row.value("rust_glancer", ReportValue::count(self.rust_glancer_count))
            .value(
                "rust_analyzer",
                ReportValue::count(self.rust_analyzer_count),
            )
            .value("matched", ReportValue::count(self.matched_count))
            .value("missing", ReportValue::count(self.missing_count))
            .value("extra", ReportValue::count(self.extra_count))
            .value("recall", optional_percent(self.recall_percent))
            .value("precision", optional_percent(self.precision_percent))
            .value(
                "unmapped_rg",
                ReportValue::count(self.rust_glancer_unmapped_count),
            )
            .value(
                "unmapped_ra",
                ReportValue::count(self.rust_analyzer_unmapped_count),
            );
    }
}

#[derive(Debug, Serialize)]
struct HoverAggregateReport {
    agreement_count: usize,
    rust_glancer_missing_count: usize,
    rust_glancer_extra_present_count: usize,
}

impl HoverAggregateReport {
    fn append_row_cells(&self, row: &mut ReportRowBuilder) {
        row.value("hover_agreements", ReportValue::count(self.agreement_count))
            .value(
                "hover_missing",
                ReportValue::count(self.rust_glancer_missing_count),
            )
            .value(
                "hover_extra_present",
                ReportValue::count(self.rust_glancer_extra_present_count),
            );
    }
}
