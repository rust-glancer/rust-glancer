//! Report data model for one public-LSP comparison run.
//!
//! The comparison layer owns the metric math. This module reshapes those typed results into a
//! serializable report and the shared document IR used by text, rich JSON, and HTML renderers.

mod aggregate;
mod fixture;
mod query;
mod server;

use std::time::Duration;

use serde::Serialize;

use crate::{
    compare_lsp::{comparison::ComparisonSummary, fixture::Fixture},
    report::{ReportDocument, ReportValue},
};

use self::{aggregate::MethodAggregateReport, fixture::FixtureReport, query::QueryReport};

pub(crate) use self::server::ServerReport;

#[derive(Debug, Serialize)]
pub(crate) struct LspComparisonReport {
    fixture: FixtureReport,
    servers: Vec<ServerReport>,
    aggregates: Vec<MethodAggregateReport>,
    queries: Vec<QueryReport>,
}

impl LspComparisonReport {
    pub(crate) fn build(
        fixture: &Fixture,
        opened_files: usize,
        rust_glancer: ServerReport,
        rust_analyzer: ServerReport,
        comparison: &ComparisonSummary,
    ) -> Self {
        Self {
            fixture: FixtureReport::capture(
                fixture,
                opened_files,
                comparison.equivalence_score_percent(),
            ),
            servers: vec![rust_glancer, rust_analyzer],
            aggregates: comparison
                .aggregates()
                .iter()
                .map(MethodAggregateReport::capture)
                .collect(),
            queries: comparison
                .queries()
                .iter()
                .map(QueryReport::capture)
                .collect(),
        }
    }

    pub(crate) fn render_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    pub(crate) fn document(&self) -> ReportDocument {
        let document = ReportDocument::builder("compare_lsp").title("rust-glancer LSP comparison");
        let document = self.fixture.append_section(document);
        let document = ServerReport::append_section(document, &self.servers);
        let document = MethodAggregateReport::append_section(document, &self.aggregates);

        QueryReport::append_sections(document, &self.queries).build()
    }
}

fn optional_percent(value: Option<f64>) -> ReportValue {
    value
        .map(ReportValue::Percent)
        .unwrap_or(ReportValue::Empty)
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}
