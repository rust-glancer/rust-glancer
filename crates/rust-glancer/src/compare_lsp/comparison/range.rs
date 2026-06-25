//! Same-document range comparison and aggregation.

use std::collections::BTreeSet;

use crate::compare_lsp::{
    comparison::{QueryComparison, QueryComparisonResult, outcome::Ratio},
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

    pub(crate) fn rust_glancer_count(&self) -> usize {
        self.rust_glancer_count
    }

    pub(crate) fn rust_analyzer_count(&self) -> usize {
        self.rust_analyzer_count
    }

    pub(crate) fn matched_count(&self) -> usize {
        self.matched.len()
    }

    pub(crate) fn missing_count(&self) -> usize {
        self.missing.len()
    }

    pub(crate) fn extra_count(&self) -> usize {
        self.extra.len()
    }

    fn completeness(&self) -> Option<Ratio> {
        Ratio::new(self.matched_count(), self.rust_analyzer_count)
    }

    fn precision_signal(&self) -> Option<Ratio> {
        Ratio::new(self.matched_count(), self.rust_glancer_count)
    }

    pub(crate) fn completeness_percent(&self) -> Option<f64> {
        self.completeness().map(Ratio::percent)
    }

    pub(crate) fn precision_signal_percent(&self) -> Option<f64> {
        self.precision_signal().map(Ratio::percent)
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
                self.rust_glancer_ranges += comparison.rust_glancer_count();
                self.rust_analyzer_ranges += comparison.rust_analyzer_count();
                self.matched_ranges += comparison.matched_count();
                self.missing_ranges += comparison.missing_count();
                self.extra_ranges += comparison.extra_count();
            }
            QueryComparisonResult::NonComparable(_) => self.non_comparable_count += 1,
            _ => {}
        }
    }

    pub(crate) fn query_count(&self) -> usize {
        self.query_count
    }

    pub(crate) fn comparable_count(&self) -> usize {
        self.comparable_count
    }

    pub(crate) fn non_comparable_count(&self) -> usize {
        self.non_comparable_count
    }

    pub(crate) fn rust_glancer_ranges(&self) -> usize {
        self.rust_glancer_ranges
    }

    pub(crate) fn rust_analyzer_ranges(&self) -> usize {
        self.rust_analyzer_ranges
    }

    pub(crate) fn matched_ranges(&self) -> usize {
        self.matched_ranges
    }

    pub(crate) fn missing_ranges(&self) -> usize {
        self.missing_ranges
    }

    pub(crate) fn extra_ranges(&self) -> usize {
        self.extra_ranges
    }

    fn weighted_completeness(&self) -> Option<Ratio> {
        Ratio::new(self.matched_ranges, self.rust_analyzer_ranges)
    }

    fn precision_signal(&self) -> Option<Ratio> {
        Ratio::new(self.matched_ranges, self.rust_glancer_ranges)
    }

    pub(crate) fn weighted_completeness_percent(&self) -> Option<f64> {
        self.weighted_completeness().map(Ratio::percent)
    }

    pub(crate) fn precision_signal_percent(&self) -> Option<f64> {
        self.precision_signal().map(Ratio::percent)
    }
}
