//! Semantic comparison over normalized LSP query outcomes.
//!
//! This layer deliberately models benchmark results as domain data rather than profiler counters.
//! The report needs per-query missing/extra locations and hover agreement, not just totals.

mod hover;
mod location;
mod outcome;

use crate::compare_lsp::{
    comparison::{
        hover::{HoverAggregate, HoverComparison},
        location::{LocationAggregate, LocationComparison},
        outcome::NonComparableComparison,
    },
    execution::ServerUnderTest,
    normalization::{NormalizedOutcome, NormalizedSummary},
    query::QueryKind,
};

/// Complete comparison result for one normalized benchmark run.
#[derive(Debug)]
pub(crate) struct ComparisonSummary {
    queries: Vec<QueryComparison>,
    aggregates: Vec<MethodAggregate>,
}

impl ComparisonSummary {
    pub(crate) fn from_normalized(normalized: &NormalizedSummary) -> Self {
        let queries = normalized
            .results()
            .iter()
            .map(QueryComparison::from_normalized)
            .collect::<Vec<_>>();
        let aggregates = MethodAggregate::from_queries(&queries);

        Self {
            queries,
            aggregates,
        }
    }

    pub(crate) fn summary_line(&self) -> String {
        format!("{} query cases compared", self.queries.len())
    }

    pub(crate) fn aggregate_summary_line(&self) -> String {
        format!(
            "aggregates=[{}]",
            self.aggregates
                .iter()
                .map(MethodAggregate::summary)
                .collect::<Vec<_>>()
                .join(", "),
        )
    }

    pub(crate) fn query_summary_line(&self) -> String {
        format!(
            "queries=[{}]",
            self.queries
                .iter()
                .map(QueryComparison::summary)
                .collect::<Vec<_>>()
                .join(", "),
        )
    }
}

#[derive(Debug)]
struct QueryComparison {
    label: &'static str,
    method: QueryMethod,
    result: QueryComparisonResult,
}

impl QueryComparison {
    fn from_normalized(
        query: &crate::compare_lsp::normalization::NormalizedQueryExecution,
    ) -> Self {
        let rust_glancer = query.outcome(ServerUnderTest::RustGlancer).value();
        let rust_analyzer = query.outcome(ServerUnderTest::RustAnalyzer).value();
        let result = match query.kind() {
            QueryKind::References { .. } | QueryKind::GotoDefinition => {
                match (rust_glancer, rust_analyzer) {
                    (
                        NormalizedOutcome::Locations(rust_glancer),
                        NormalizedOutcome::Locations(rust_analyzer),
                    ) => QueryComparisonResult::Locations(LocationComparison::new(
                        rust_glancer,
                        rust_analyzer,
                    )),
                    _ => QueryComparisonResult::NonComparable(NonComparableComparison::new(
                        rust_glancer,
                        rust_analyzer,
                    )),
                }
            }
            QueryKind::Hover => match (rust_glancer, rust_analyzer) {
                (
                    NormalizedOutcome::Hover {
                        present: rust_glancer_present,
                    },
                    NormalizedOutcome::Hover {
                        present: rust_analyzer_present,
                    },
                ) => QueryComparisonResult::Hover(HoverComparison::new(
                    *rust_glancer_present,
                    *rust_analyzer_present,
                )),
                _ => QueryComparisonResult::NonComparable(NonComparableComparison::new(
                    rust_glancer,
                    rust_analyzer,
                )),
            },
        };

        Self {
            label: query.label(),
            method: QueryMethod::from_kind(query.kind()),
            result,
        }
    }

    fn result(&self) -> &QueryComparisonResult {
        &self.result
    }

    fn summary(&self) -> String {
        format!(
            "{}:{}={}",
            self.method.label(),
            compact(self.label),
            self.result.summary(),
        )
    }
}

#[derive(Debug)]
enum QueryComparisonResult {
    Locations(LocationComparison),
    Hover(HoverComparison),
    NonComparable(NonComparableComparison),
}

impl QueryComparisonResult {
    fn summary(&self) -> String {
        match self {
            Self::Locations(comparison) => comparison.summary(),
            Self::Hover(comparison) => comparison.summary(),
            Self::NonComparable(comparison) => comparison.summary(),
        }
    }
}

#[derive(Debug)]
struct MethodAggregate {
    method: QueryMethod,
    data: MethodAggregateData,
}

impl MethodAggregate {
    fn from_queries(queries: &[QueryComparison]) -> Vec<Self> {
        let mut references = LocationAggregate::default();
        let mut goto_definition = LocationAggregate::default();
        let mut hover = HoverAggregate::default();

        for query in queries {
            match query.method {
                QueryMethod::References => references.record(query),
                QueryMethod::GotoDefinition => goto_definition.record(query),
                QueryMethod::Hover => hover.record(query),
            }
        }

        let mut aggregates = Vec::new();
        if references.query_count() > 0 {
            aggregates.push(Self {
                method: QueryMethod::References,
                data: MethodAggregateData::Locations(references),
            });
        }
        if goto_definition.query_count() > 0 {
            aggregates.push(Self {
                method: QueryMethod::GotoDefinition,
                data: MethodAggregateData::Locations(goto_definition),
            });
        }
        if hover.query_count() > 0 {
            aggregates.push(Self {
                method: QueryMethod::Hover,
                data: MethodAggregateData::Hover(hover),
            });
        }

        aggregates
    }

    fn summary(&self) -> String {
        match &self.data {
            MethodAggregateData::Locations(aggregate) => {
                format!("{}: {}", self.method.label(), aggregate.summary())
            }
            MethodAggregateData::Hover(aggregate) => {
                format!("{}: {}", self.method.label(), aggregate.summary())
            }
        }
    }
}

#[derive(Debug)]
enum MethodAggregateData {
    Locations(LocationAggregate),
    Hover(HoverAggregate),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QueryMethod {
    References,
    GotoDefinition,
    Hover,
}

impl QueryMethod {
    fn from_kind(kind: QueryKind) -> Self {
        match kind {
            QueryKind::References { .. } => Self::References,
            QueryKind::GotoDefinition => Self::GotoDefinition,
            QueryKind::Hover => Self::Hover,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::References => "references",
            Self::GotoDefinition => "goto_definition",
            Self::Hover => "hover",
        }
    }
}

fn compact(message: &str) -> String {
    let mut message = message.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX_LEN: usize = 80;
    if message.len() > MAX_LEN {
        message.truncate(MAX_LEN);
        message.push_str("...");
    }
    message
}

#[cfg(test)]
mod tests {
    use crate::compare_lsp::{
        comparison::{ComparisonSummary, MethodAggregateData, QueryComparisonResult, QueryMethod},
        normalization::{
            NormalizedLocation, NormalizedLocationSet, NormalizedOutcome, NormalizedQueryExecution,
            NormalizedRange, NormalizedSummary,
        },
        query::QueryKind,
    };

    #[test]
    fn compares_location_sets_and_aggregates_by_method() {
        let shared = location("src/lib.rs", 1);
        let extra = location("src/extra.rs", 2);
        let missing = location("src/missing.rs", 3);
        let summary = NormalizedSummary::test_from_results(vec![
            NormalizedQueryExecution::test_new(
                "references",
                QueryKind::References {
                    include_declaration: true,
                },
                locations(vec![shared.clone(), extra.clone()]),
                locations(vec![shared.clone(), missing.clone()]),
            ),
            NormalizedQueryExecution::test_new(
                "references error",
                QueryKind::References {
                    include_declaration: true,
                },
                NormalizedOutcome::Error {
                    code: -32603,
                    message: "analysis failed".to_string(),
                },
                locations(vec![shared.clone()]),
            ),
        ]);

        let comparison = ComparisonSummary::from_normalized(&summary);

        assert_eq!(comparison.queries.len(), 2);
        let QueryComparisonResult::Locations(query) = &comparison.queries[0].result else {
            panic!("first query should compare locations");
        };
        assert_eq!(query.rust_glancer_count(), 2);
        assert_eq!(query.rust_analyzer_count(), 2);
        assert_eq!(query.matched(), &[shared]);
        assert_eq!(query.missing(), &[missing]);
        assert_eq!(query.extra(), &[extra]);
        assert_eq!(
            query.completeness().map(|ratio| ratio.percent()),
            Some(50.0),
        );
        assert_eq!(
            query.precision_signal().map(|ratio| ratio.percent()),
            Some(50.0),
        );

        let references = comparison
            .aggregates
            .iter()
            .find(|aggregate| aggregate.method == QueryMethod::References)
            .expect("references aggregate should be present");
        let MethodAggregateData::Locations(aggregate) = &references.data else {
            panic!("references should use location aggregation");
        };
        assert_eq!(aggregate.query_count(), 2);
        assert_eq!(aggregate.comparable_count(), 1);
        assert_eq!(aggregate.non_comparable_count(), 1);
        assert_eq!(aggregate.rust_glancer_locations(), 2);
        assert_eq!(aggregate.rust_analyzer_locations(), 2);
        assert_eq!(aggregate.matched_locations(), 1);
        assert_eq!(aggregate.missing_locations(), 1);
        assert_eq!(aggregate.extra_locations(), 1);
        assert_eq!(
            aggregate
                .weighted_completeness()
                .map(|ratio| ratio.percent()),
            Some(50.0),
        );
    }

    #[test]
    fn compares_hover_agreement_and_mismatches() {
        let summary = NormalizedSummary::test_from_results(vec![
            NormalizedQueryExecution::test_new(
                "hover agreement",
                QueryKind::Hover,
                NormalizedOutcome::Hover { present: true },
                NormalizedOutcome::Hover { present: true },
            ),
            NormalizedQueryExecution::test_new(
                "hover missing",
                QueryKind::Hover,
                NormalizedOutcome::Hover { present: false },
                NormalizedOutcome::Hover { present: true },
            ),
            NormalizedQueryExecution::test_new(
                "hover timeout",
                QueryKind::Hover,
                NormalizedOutcome::Timeout,
                NormalizedOutcome::Hover { present: true },
            ),
        ]);

        let comparison = ComparisonSummary::from_normalized(&summary);

        let hover = comparison
            .aggregates
            .iter()
            .find(|aggregate| aggregate.method == QueryMethod::Hover)
            .expect("hover aggregate should be present");
        let MethodAggregateData::Hover(aggregate) = &hover.data else {
            panic!("hover should use hover aggregation");
        };
        assert_eq!(aggregate.query_count(), 3);
        assert_eq!(aggregate.comparable_count(), 2);
        assert_eq!(aggregate.agreement_count(), 1);
        assert_eq!(aggregate.rust_glancer_missing_count(), 1);
        assert_eq!(aggregate.rust_glancer_extra_present_count(), 0);
        assert_eq!(aggregate.non_comparable_count(), 1);
    }

    fn locations(locations: Vec<NormalizedLocation>) -> NormalizedOutcome {
        NormalizedOutcome::Locations(NormalizedLocationSet::test_from_locations(locations))
    }

    fn location(path: &'static str, line: u32) -> NormalizedLocation {
        NormalizedLocation::test_new(path, NormalizedRange::test_new(line, 0, line, 4))
    }
}
