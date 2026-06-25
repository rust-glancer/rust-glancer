//! Symbol-result comparison and aggregation.

use std::collections::BTreeSet;

use super::metrics::{
    AggregateSummaryMetrics, MappedSetAggregateMetrics, MappedSetComparisonMetrics,
    SetComparisonMetrics,
};
use crate::compare_lsp::{
    comparison::{QueryComparison, QueryComparisonResult},
    normalization::{NormalizedSymbol, NormalizedSymbolSet},
};

#[derive(Debug)]
pub(crate) struct SymbolComparison {
    rust_glancer_count: usize,
    rust_analyzer_count: usize,
    matched: Vec<NormalizedSymbol>,
    missing: Vec<NormalizedSymbol>,
    extra: Vec<NormalizedSymbol>,
    rust_glancer_unmapped_count: usize,
    rust_analyzer_unmapped_count: usize,
    rust_glancer_unmapped: Vec<String>,
    rust_analyzer_unmapped: Vec<String>,
}

impl SymbolComparison {
    pub(super) fn new(
        rust_glancer: &NormalizedSymbolSet,
        rust_analyzer: &NormalizedSymbolSet,
    ) -> Self {
        let rust_glancer_symbols = rust_glancer
            .symbols()
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let rust_analyzer_symbols = rust_analyzer
            .symbols()
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();

        let matched = rust_glancer_symbols
            .intersection(&rust_analyzer_symbols)
            .cloned()
            .collect();
        let missing = rust_analyzer_symbols
            .difference(&rust_glancer_symbols)
            .cloned()
            .collect();
        let extra = rust_glancer_symbols
            .difference(&rust_analyzer_symbols)
            .cloned()
            .collect();

        Self {
            rust_glancer_count: rust_glancer_symbols.len(),
            rust_analyzer_count: rust_analyzer_symbols.len(),
            matched,
            missing,
            extra,
            rust_glancer_unmapped_count: rust_glancer.unmapped_count(),
            rust_analyzer_unmapped_count: rust_analyzer.unmapped_count(),
            rust_glancer_unmapped: rust_glancer.unmapped_summaries(),
            rust_analyzer_unmapped: rust_analyzer.unmapped_summaries(),
        }
    }

    pub(crate) fn metrics(&self) -> MappedSetComparisonMetrics {
        MappedSetComparisonMetrics {
            set: SetComparisonMetrics::new(
                self.rust_glancer_count,
                self.rust_analyzer_count,
                self.matched.len(),
                self.missing.len(),
                self.extra.len(),
            ),
            rust_glancer_unmapped_count: self.rust_glancer_unmapped_count,
            rust_analyzer_unmapped_count: self.rust_analyzer_unmapped_count,
            rust_glancer_unmapped: self.rust_glancer_unmapped.clone(),
            rust_analyzer_unmapped: self.rust_analyzer_unmapped.clone(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct SymbolAggregate {
    query_count: usize,
    comparable_count: usize,
    non_comparable_count: usize,
    rust_glancer_symbols: usize,
    rust_analyzer_symbols: usize,
    matched_symbols: usize,
    missing_symbols: usize,
    extra_symbols: usize,
    rust_glancer_unmapped_symbols: usize,
    rust_analyzer_unmapped_symbols: usize,
}

impl SymbolAggregate {
    pub(super) fn record(&mut self, query: &QueryComparison) {
        self.query_count += 1;
        match query.result() {
            QueryComparisonResult::Symbols(comparison) => {
                self.comparable_count += 1;
                self.rust_glancer_symbols += comparison.rust_glancer_count;
                self.rust_analyzer_symbols += comparison.rust_analyzer_count;
                self.matched_symbols += comparison.matched.len();
                self.missing_symbols += comparison.missing.len();
                self.extra_symbols += comparison.extra.len();
                self.rust_glancer_unmapped_symbols += comparison.rust_glancer_unmapped_count;
                self.rust_analyzer_unmapped_symbols += comparison.rust_analyzer_unmapped_count;
            }
            QueryComparisonResult::NonComparable(_) => self.non_comparable_count += 1,
            _ => {}
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.query_count == 0
    }

    pub(super) fn summary(&self) -> AggregateSummaryMetrics {
        AggregateSummaryMetrics {
            query_count: self.query_count,
            comparable_count: self.comparable_count,
            non_comparable_count: self.non_comparable_count,
        }
    }

    pub(crate) fn metrics(&self) -> MappedSetAggregateMetrics {
        MappedSetAggregateMetrics {
            set: SetComparisonMetrics::new(
                self.rust_glancer_symbols,
                self.rust_analyzer_symbols,
                self.matched_symbols,
                self.missing_symbols,
                self.extra_symbols,
            ),
            rust_glancer_unmapped_count: self.rust_glancer_unmapped_symbols,
            rust_analyzer_unmapped_count: self.rust_analyzer_unmapped_symbols,
        }
    }
}
