//! Aggregate comparison report section.

use serde::Serialize;

use crate::{
    compare_lsp::comparison::{
        MappedSetAggregateMetrics, MethodAggregate, MethodAggregateData, SetComparisonMetrics,
    },
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
        let summary = aggregate.data().summary();
        Self {
            method: aggregate.method().lsp_method().to_string(),
            query_count: summary.query_count,
            comparable_count: summary.comparable_count,
            non_comparable_count: summary.non_comparable_count,
            data: MethodAggregateDataReport::capture(aggregate.data()),
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
                "match_score",
                "Match score",
                ReportAlign::Right,
                Some(ReportUnit::Percent),
            )
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
    PrepareRenames(RangeAggregateReport),
    RenameEdits(LocationAggregateReport),
    Ranges(RangeAggregateReport),
    Symbols(SymbolAggregateReport),
    InlayHints(RangeAggregateReport),
    Hover(RangeAggregateReport),
}

impl MethodAggregateDataReport {
    fn capture(data: &MethodAggregateData) -> Self {
        match data {
            MethodAggregateData::Locations(locations) => {
                Self::Locations(locations.metrics().into())
            }
            MethodAggregateData::PrepareRenames(rename) => {
                Self::PrepareRenames(rename.metrics().into())
            }
            MethodAggregateData::RenameEdits(rename) => Self::RenameEdits(rename.metrics().into()),
            MethodAggregateData::Ranges(ranges) => Self::Ranges(ranges.metrics().into()),
            MethodAggregateData::Symbols(symbols) => Self::Symbols(symbols.metrics().into()),
            MethodAggregateData::InlayHints(hints) => Self::InlayHints(hints.metrics().into()),
            MethodAggregateData::Hover(hover) => Self::Hover(hover.metrics().into()),
        }
    }

    fn append_row_cells(&self, row: &mut ReportRowBuilder) {
        match self {
            Self::Locations(locations) => locations.append_row_cells(row),
            Self::PrepareRenames(rename) => rename.append_row_cells(row),
            Self::RenameEdits(rename) => rename.append_row_cells(row),
            Self::Ranges(ranges) => ranges.append_row_cells(row),
            Self::Symbols(symbols) => symbols.append_row_cells(row),
            Self::InlayHints(hints) => hints.append_row_cells(row),
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
    match_score_percent: f64,
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
            .value(
                "match_score",
                ReportValue::Percent(self.match_score_percent),
            )
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

impl From<MappedSetAggregateMetrics> for LocationAggregateReport {
    fn from(metrics: MappedSetAggregateMetrics) -> Self {
        Self {
            rust_glancer_count: metrics.set.rust_glancer_count,
            rust_analyzer_count: metrics.set.rust_analyzer_count,
            matched_count: metrics.set.matched_count,
            missing_count: metrics.set.missing_count,
            extra_count: metrics.set.extra_count,
            rust_glancer_unmapped_count: metrics.rust_glancer_unmapped_count,
            rust_analyzer_unmapped_count: metrics.rust_analyzer_unmapped_count,
            match_score_percent: metrics.set.match_score_percent,
            recall_percent: metrics.set.recall_percent,
            precision_percent: metrics.set.precision_percent,
        }
    }
}

#[derive(Debug, Serialize)]
struct RangeAggregateReport {
    rust_glancer_count: usize,
    rust_analyzer_count: usize,
    matched_count: usize,
    missing_count: usize,
    extra_count: usize,
    match_score_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    recall_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    precision_percent: Option<f64>,
}

impl RangeAggregateReport {
    fn append_row_cells(&self, row: &mut ReportRowBuilder) {
        row.value("rust_glancer", ReportValue::count(self.rust_glancer_count))
            .value(
                "rust_analyzer",
                ReportValue::count(self.rust_analyzer_count),
            )
            .value("matched", ReportValue::count(self.matched_count))
            .value("missing", ReportValue::count(self.missing_count))
            .value("extra", ReportValue::count(self.extra_count))
            .value(
                "match_score",
                ReportValue::Percent(self.match_score_percent),
            )
            .value("recall", optional_percent(self.recall_percent))
            .value("precision", optional_percent(self.precision_percent));
    }
}

impl From<SetComparisonMetrics> for RangeAggregateReport {
    fn from(metrics: SetComparisonMetrics) -> Self {
        Self {
            rust_glancer_count: metrics.rust_glancer_count,
            rust_analyzer_count: metrics.rust_analyzer_count,
            matched_count: metrics.matched_count,
            missing_count: metrics.missing_count,
            extra_count: metrics.extra_count,
            match_score_percent: metrics.match_score_percent,
            recall_percent: metrics.recall_percent,
            precision_percent: metrics.precision_percent,
        }
    }
}

#[derive(Debug, Serialize)]
struct SymbolAggregateReport {
    rust_glancer_count: usize,
    rust_analyzer_count: usize,
    matched_count: usize,
    missing_count: usize,
    extra_count: usize,
    rust_glancer_unmapped_count: usize,
    rust_analyzer_unmapped_count: usize,
    match_score_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    recall_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    precision_percent: Option<f64>,
}

impl SymbolAggregateReport {
    fn append_row_cells(&self, row: &mut ReportRowBuilder) {
        row.value("rust_glancer", ReportValue::count(self.rust_glancer_count))
            .value(
                "rust_analyzer",
                ReportValue::count(self.rust_analyzer_count),
            )
            .value("matched", ReportValue::count(self.matched_count))
            .value("missing", ReportValue::count(self.missing_count))
            .value("extra", ReportValue::count(self.extra_count))
            .value(
                "match_score",
                ReportValue::Percent(self.match_score_percent),
            )
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

impl From<MappedSetAggregateMetrics> for SymbolAggregateReport {
    fn from(metrics: MappedSetAggregateMetrics) -> Self {
        Self {
            rust_glancer_count: metrics.set.rust_glancer_count,
            rust_analyzer_count: metrics.set.rust_analyzer_count,
            matched_count: metrics.set.matched_count,
            missing_count: metrics.set.missing_count,
            extra_count: metrics.set.extra_count,
            rust_glancer_unmapped_count: metrics.rust_glancer_unmapped_count,
            rust_analyzer_unmapped_count: metrics.rust_analyzer_unmapped_count,
            match_score_percent: metrics.set.match_score_percent,
            recall_percent: metrics.set.recall_percent,
            precision_percent: metrics.set.precision_percent,
        }
    }
}
