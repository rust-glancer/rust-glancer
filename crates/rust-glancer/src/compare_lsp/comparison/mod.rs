//! Semantic comparison over normalized LSP query outcomes.
//!
//! This layer deliberately models benchmark results as domain data rather than profiler counters.
//! The report needs per-query missing/extra locations and hover agreement, not just totals.

mod hover;
mod inlay_hint;
mod location;
mod metrics;
mod outcome;
mod range;
mod rename;
mod symbol;

use std::time::Duration;

use crate::compare_lsp::{
    comparison::{
        hover::{HoverAggregate, HoverComparison},
        inlay_hint::{InlayHintAggregate, InlayHintComparison},
        location::{LocationAggregate, LocationComparison},
        outcome::NonComparableComparison,
        range::{RangeAggregate, RangeComparison},
        rename::{
            PrepareRenameAggregate, PrepareRenameComparison, RenameEditAggregate,
            RenameEditComparison,
        },
        symbol::{SymbolAggregate, SymbolComparison},
    },
    execution::ServerUnderTest,
    normalization::{NormalizedOutcome, NormalizedSummary},
    query::QueryKind,
};

pub(crate) use self::metrics::{
    AggregateSummaryMetrics, HoverAggregateMetrics, HoverComparisonMetrics,
    MappedSetAggregateMetrics, MappedSetComparisonMetrics, NonComparableMetrics,
    SetComparisonMetrics,
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

    pub(crate) fn queries(&self) -> &[QueryComparison] {
        &self.queries
    }

    pub(crate) fn aggregates(&self) -> &[MethodAggregate] {
        &self.aggregates
    }
}

#[derive(Debug)]
pub(crate) struct QueryComparison {
    label: &'static str,
    method: QueryMethod,
    rust_glancer_latency: Duration,
    rust_analyzer_latency: Duration,
    result: QueryComparisonResult,
}

impl QueryComparison {
    fn from_normalized(
        query: &crate::compare_lsp::normalization::NormalizedQueryExecution,
    ) -> Self {
        let rust_glancer = query.outcome(ServerUnderTest::RustGlancer).value();
        let rust_analyzer = query.outcome(ServerUnderTest::RustAnalyzer).value();
        let result = match query.kind() {
            QueryKind::References { .. }
            | QueryKind::GotoDefinition
            | QueryKind::TypeDefinition
            | QueryKind::Implementation => match (rust_glancer, rust_analyzer) {
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
            },
            QueryKind::PrepareRename => match (rust_glancer, rust_analyzer) {
                (
                    NormalizedOutcome::PrepareRenames(rust_glancer),
                    NormalizedOutcome::PrepareRenames(rust_analyzer),
                ) => QueryComparisonResult::PrepareRenames(PrepareRenameComparison::new(
                    rust_glancer,
                    rust_analyzer,
                )),
                _ => QueryComparisonResult::NonComparable(NonComparableComparison::new(
                    rust_glancer,
                    rust_analyzer,
                )),
            },
            QueryKind::Rename => match (rust_glancer, rust_analyzer) {
                (
                    NormalizedOutcome::RenameEdits(rust_glancer),
                    NormalizedOutcome::RenameEdits(rust_analyzer),
                ) => QueryComparisonResult::RenameEdits(RenameEditComparison::new(
                    rust_glancer,
                    rust_analyzer,
                )),
                _ => QueryComparisonResult::NonComparable(NonComparableComparison::new(
                    rust_glancer,
                    rust_analyzer,
                )),
            },
            QueryKind::DocumentHighlight => match (rust_glancer, rust_analyzer) {
                (
                    NormalizedOutcome::Ranges(rust_glancer),
                    NormalizedOutcome::Ranges(rust_analyzer),
                ) => {
                    QueryComparisonResult::Ranges(RangeComparison::new(rust_glancer, rust_analyzer))
                }
                _ => QueryComparisonResult::NonComparable(NonComparableComparison::new(
                    rust_glancer,
                    rust_analyzer,
                )),
            },
            QueryKind::DocumentSymbol | QueryKind::WorkspaceSymbol => {
                match (rust_glancer, rust_analyzer) {
                    (
                        NormalizedOutcome::Symbols(rust_glancer),
                        NormalizedOutcome::Symbols(rust_analyzer),
                    ) => QueryComparisonResult::Symbols(SymbolComparison::new(
                        rust_glancer,
                        rust_analyzer,
                    )),
                    _ => QueryComparisonResult::NonComparable(NonComparableComparison::new(
                        rust_glancer,
                        rust_analyzer,
                    )),
                }
            }
            QueryKind::InlayHint => match (rust_glancer, rust_analyzer) {
                (
                    NormalizedOutcome::InlayHints(rust_glancer),
                    NormalizedOutcome::InlayHints(rust_analyzer),
                ) => QueryComparisonResult::InlayHints(InlayHintComparison::new(
                    rust_glancer,
                    rust_analyzer,
                )),
                _ => QueryComparisonResult::NonComparable(NonComparableComparison::new(
                    rust_glancer,
                    rust_analyzer,
                )),
            },
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
            rust_glancer_latency: query.outcome(ServerUnderTest::RustGlancer).latency(),
            rust_analyzer_latency: query.outcome(ServerUnderTest::RustAnalyzer).latency(),
            result,
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        self.label
    }

    pub(crate) fn method(&self) -> QueryMethod {
        self.method
    }

    pub(crate) fn rust_glancer_latency(&self) -> Duration {
        self.rust_glancer_latency
    }

    pub(crate) fn rust_analyzer_latency(&self) -> Duration {
        self.rust_analyzer_latency
    }

    pub(crate) fn result(&self) -> &QueryComparisonResult {
        &self.result
    }
}

#[derive(Debug)]
pub(crate) enum QueryComparisonResult {
    Locations(LocationComparison),
    PrepareRenames(PrepareRenameComparison),
    RenameEdits(RenameEditComparison),
    Ranges(RangeComparison),
    Symbols(SymbolComparison),
    InlayHints(InlayHintComparison),
    Hover(HoverComparison),
    NonComparable(NonComparableComparison),
}

#[derive(Debug)]
pub(crate) struct MethodAggregate {
    method: QueryMethod,
    data: MethodAggregateData,
}

impl MethodAggregate {
    fn from_queries(queries: &[QueryComparison]) -> Vec<Self> {
        let mut references = LocationAggregate::default();
        let mut goto_definition = LocationAggregate::default();
        let mut type_definition = LocationAggregate::default();
        let mut implementation = LocationAggregate::default();
        let mut prepare_rename = PrepareRenameAggregate::default();
        let mut rename = RenameEditAggregate::default();
        let mut document_highlight = RangeAggregate::default();
        let mut document_symbol = SymbolAggregate::default();
        let mut workspace_symbol = SymbolAggregate::default();
        let mut inlay_hint = InlayHintAggregate::default();
        let mut hover = HoverAggregate::default();

        for query in queries {
            match query.method {
                QueryMethod::References => references.record(query),
                QueryMethod::GotoDefinition => goto_definition.record(query),
                QueryMethod::TypeDefinition => type_definition.record(query),
                QueryMethod::Implementation => implementation.record(query),
                QueryMethod::PrepareRename => prepare_rename.record(query),
                QueryMethod::Rename => rename.record(query),
                QueryMethod::DocumentHighlight => document_highlight.record(query),
                QueryMethod::DocumentSymbol => document_symbol.record(query),
                QueryMethod::WorkspaceSymbol => workspace_symbol.record(query),
                QueryMethod::InlayHint => inlay_hint.record(query),
                QueryMethod::Hover => hover.record(query),
            }
        }

        let mut aggregates = Vec::new();
        if !references.is_empty() {
            aggregates.push(Self {
                method: QueryMethod::References,
                data: MethodAggregateData::Locations(references),
            });
        }
        if !goto_definition.is_empty() {
            aggregates.push(Self {
                method: QueryMethod::GotoDefinition,
                data: MethodAggregateData::Locations(goto_definition),
            });
        }
        if !type_definition.is_empty() {
            aggregates.push(Self {
                method: QueryMethod::TypeDefinition,
                data: MethodAggregateData::Locations(type_definition),
            });
        }
        if !implementation.is_empty() {
            aggregates.push(Self {
                method: QueryMethod::Implementation,
                data: MethodAggregateData::Locations(implementation),
            });
        }
        if !prepare_rename.is_empty() {
            aggregates.push(Self {
                method: QueryMethod::PrepareRename,
                data: MethodAggregateData::PrepareRenames(prepare_rename),
            });
        }
        if !rename.is_empty() {
            aggregates.push(Self {
                method: QueryMethod::Rename,
                data: MethodAggregateData::RenameEdits(rename),
            });
        }
        if !document_highlight.is_empty() {
            aggregates.push(Self {
                method: QueryMethod::DocumentHighlight,
                data: MethodAggregateData::Ranges(document_highlight),
            });
        }
        if !document_symbol.is_empty() {
            aggregates.push(Self {
                method: QueryMethod::DocumentSymbol,
                data: MethodAggregateData::Symbols(document_symbol),
            });
        }
        if !workspace_symbol.is_empty() {
            aggregates.push(Self {
                method: QueryMethod::WorkspaceSymbol,
                data: MethodAggregateData::Symbols(workspace_symbol),
            });
        }
        if !inlay_hint.is_empty() {
            aggregates.push(Self {
                method: QueryMethod::InlayHint,
                data: MethodAggregateData::InlayHints(inlay_hint),
            });
        }
        if !hover.is_empty() {
            aggregates.push(Self {
                method: QueryMethod::Hover,
                data: MethodAggregateData::Hover(hover),
            });
        }

        aggregates
    }

    pub(crate) fn method(&self) -> QueryMethod {
        self.method
    }

    pub(crate) fn data(&self) -> &MethodAggregateData {
        &self.data
    }
}

#[derive(Debug)]
pub(crate) enum MethodAggregateData {
    Locations(LocationAggregate),
    PrepareRenames(PrepareRenameAggregate),
    RenameEdits(RenameEditAggregate),
    Ranges(RangeAggregate),
    Symbols(SymbolAggregate),
    InlayHints(InlayHintAggregate),
    Hover(HoverAggregate),
}

impl MethodAggregateData {
    pub(crate) fn summary(&self) -> AggregateSummaryMetrics {
        match self {
            Self::Locations(locations) => locations.summary(),
            Self::PrepareRenames(rename) => rename.summary(),
            Self::RenameEdits(rename) => rename.summary(),
            Self::Ranges(ranges) => ranges.summary(),
            Self::Symbols(symbols) => symbols.summary(),
            Self::InlayHints(hints) => hints.summary(),
            Self::Hover(hover) => hover.summary(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueryMethod {
    References,
    GotoDefinition,
    TypeDefinition,
    Implementation,
    PrepareRename,
    Rename,
    DocumentHighlight,
    DocumentSymbol,
    WorkspaceSymbol,
    InlayHint,
    Hover,
}

impl QueryMethod {
    fn from_kind(kind: QueryKind) -> Self {
        match kind {
            QueryKind::References { .. } => Self::References,
            QueryKind::GotoDefinition => Self::GotoDefinition,
            QueryKind::TypeDefinition => Self::TypeDefinition,
            QueryKind::Implementation => Self::Implementation,
            QueryKind::PrepareRename => Self::PrepareRename,
            QueryKind::Rename => Self::Rename,
            QueryKind::DocumentHighlight => Self::DocumentHighlight,
            QueryKind::DocumentSymbol => Self::DocumentSymbol,
            QueryKind::WorkspaceSymbol => Self::WorkspaceSymbol,
            QueryKind::InlayHint => Self::InlayHint,
            QueryKind::Hover => Self::Hover,
        }
    }

    pub(crate) fn lsp_method(self) -> &'static str {
        match self {
            Self::References => QueryKind::References {
                include_declaration: true,
            }
            .lsp_method(),
            Self::GotoDefinition => QueryKind::GotoDefinition.lsp_method(),
            Self::TypeDefinition => QueryKind::TypeDefinition.lsp_method(),
            Self::Implementation => QueryKind::Implementation.lsp_method(),
            Self::PrepareRename => QueryKind::PrepareRename.lsp_method(),
            Self::Rename => QueryKind::Rename.lsp_method(),
            Self::DocumentHighlight => QueryKind::DocumentHighlight.lsp_method(),
            Self::DocumentSymbol => QueryKind::DocumentSymbol.lsp_method(),
            Self::WorkspaceSymbol => QueryKind::WorkspaceSymbol.lsp_method(),
            Self::InlayHint => QueryKind::InlayHint.lsp_method(),
            Self::Hover => QueryKind::Hover.lsp_method(),
        }
    }
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
        let query_metrics = query.metrics();
        assert_eq!(query_metrics.set.rust_glancer_count, 2);
        assert_eq!(query_metrics.set.rust_analyzer_count, 2);
        assert_eq!(query.matched(), &[shared]);
        assert_eq!(query.missing(), &[missing]);
        assert_eq!(query.extra(), &[extra]);
        assert_eq!(query_metrics.set.recall_percent, Some(50.0));
        assert_eq!(query_metrics.set.precision_percent, Some(50.0));
        assert_eq!(query_metrics.set.match_score_percent, 50.0);

        let references = comparison
            .aggregates
            .iter()
            .find(|aggregate| aggregate.method == QueryMethod::References)
            .expect("references aggregate should be present");
        let MethodAggregateData::Locations(aggregate) = &references.data else {
            panic!("references should use location aggregation");
        };
        let summary = aggregate.summary();
        let metrics = aggregate.metrics();
        assert_eq!(summary.query_count, 2);
        assert_eq!(summary.comparable_count, 1);
        assert_eq!(summary.non_comparable_count, 1);
        assert_eq!(metrics.set.rust_glancer_count, 2);
        assert_eq!(metrics.set.rust_analyzer_count, 2);
        assert_eq!(metrics.set.matched_count, 1);
        assert_eq!(metrics.set.missing_count, 1);
        assert_eq!(metrics.set.extra_count, 1);
        assert_eq!(metrics.set.recall_percent, Some(50.0),);
        assert_eq!(metrics.set.match_score_percent, 50.0);
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
        let summary = aggregate.summary();
        let metrics = aggregate.metrics();
        assert_eq!(summary.query_count, 3);
        assert_eq!(summary.comparable_count, 2);
        assert_eq!(metrics.agreement_count, 1);
        assert_eq!(metrics.rust_glancer_missing_count, 1);
        assert_eq!(metrics.rust_glancer_extra_present_count, 0);
        assert_eq!(summary.non_comparable_count, 1);
    }

    fn locations(locations: Vec<NormalizedLocation>) -> NormalizedOutcome {
        NormalizedOutcome::Locations(NormalizedLocationSet::test_from_locations(locations))
    }

    fn location(path: &'static str, line: u32) -> NormalizedLocation {
        NormalizedLocation::test_new(path, NormalizedRange::test_new(line, 0, line, 4))
    }
}
