//! Same-document range comparison and aggregation.

use std::collections::BTreeSet;

use super::metrics::{AggregateSummaryMetrics, SetComparisonMetrics};
use crate::compare_lsp::{
    comparison::{QueryComparison, QueryComparisonResult},
    normalization::{NormalizedRange, NormalizedRangeSet},
};

#[derive(Debug)]
pub(crate) struct RangeComparison {
    rust_glancer_count: usize,
    rust_analyzer_count: usize,
    matched: Vec<NormalizedRange>,
    missing: Vec<NormalizedRange>,
    extra: Vec<NormalizedRange>,
}

impl RangeComparison {
    pub(super) fn new(
        rust_glancer: &NormalizedRangeSet,
        rust_analyzer: &NormalizedRangeSet,
    ) -> Self {
        let rust_glancer_ranges = rust_glancer
            .ranges()
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let rust_analyzer_ranges = rust_analyzer
            .ranges()
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();

        // Highlights are scoped to the requested document, so the range itself is the stable
        // comparable unit. Kind is deliberately ignored for now; engines often differ on read/write
        // classification while still finding the same occurrences.
        let matched = rust_glancer_ranges
            .intersection(&rust_analyzer_ranges)
            .copied()
            .collect();
        let missing = rust_analyzer_ranges
            .difference(&rust_glancer_ranges)
            .copied()
            .collect();
        let extra = rust_glancer_ranges
            .difference(&rust_analyzer_ranges)
            .copied()
            .collect();

        Self {
            rust_glancer_count: rust_glancer_ranges.len(),
            rust_analyzer_count: rust_analyzer_ranges.len(),
            matched,
            missing,
            extra,
        }
    }

    pub(crate) fn metrics(&self) -> SetComparisonMetrics {
        SetComparisonMetrics::new(
            self.rust_glancer_count,
            self.rust_analyzer_count,
            self.matched.len(),
            self.missing.len(),
            self.extra.len(),
        )
    }
}

#[derive(Debug, Default)]
pub(crate) struct RangeAggregate {
    query_count: usize,
    comparable_count: usize,
    non_comparable_count: usize,
    rust_glancer_ranges: usize,
    rust_analyzer_ranges: usize,
    matched_ranges: usize,
    missing_ranges: usize,
    extra_ranges: usize,
}

impl RangeAggregate {
    pub(super) fn record(&mut self, query: &QueryComparison) {
        self.query_count += 1;
        match query.result() {
            QueryComparisonResult::Ranges(comparison) => {
                self.comparable_count += 1;
                self.rust_glancer_ranges += comparison.rust_glancer_count;
                self.rust_analyzer_ranges += comparison.rust_analyzer_count;
                self.matched_ranges += comparison.matched.len();
                self.missing_ranges += comparison.missing.len();
                self.extra_ranges += comparison.extra.len();
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

    pub(crate) fn metrics(&self) -> SetComparisonMetrics {
        SetComparisonMetrics::new(
            self.rust_glancer_ranges,
            self.rust_analyzer_ranges,
            self.matched_ranges,
            self.missing_ranges,
            self.extra_ranges,
        )
    }
}
