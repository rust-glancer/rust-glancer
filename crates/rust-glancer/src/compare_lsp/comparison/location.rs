//! Location-result comparison and aggregation.

use std::collections::BTreeSet;

use crate::compare_lsp::{
    comparison::{QueryComparison, QueryComparisonResult, outcome::Ratio},
    normalization::{NormalizedLocation, NormalizedLocationSet},
};

#[derive(Debug)]
pub(crate) struct LocationComparison {
    rust_glancer_count: usize,
    rust_analyzer_count: usize,
    matched: Vec<NormalizedLocation>,
    missing: Vec<NormalizedLocation>,
    extra: Vec<NormalizedLocation>,
    rust_glancer_unmapped_count: usize,
    rust_analyzer_unmapped_count: usize,
    rust_glancer_unmapped: Vec<String>,
    rust_analyzer_unmapped: Vec<String>,
}

impl LocationComparison {
    pub(super) fn new(
        rust_glancer: &NormalizedLocationSet,
        rust_analyzer: &NormalizedLocationSet,
    ) -> Self {
        let rust_glancer_locations = rust_glancer
            .locations()
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let rust_analyzer_locations = rust_analyzer
            .locations()
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();

        // The normalized lists are already deterministic, but using set operations here makes the
        // scoring rules explicit and keeps missing/extra details ready for the report slice.
        let matched = rust_glancer_locations
            .intersection(&rust_analyzer_locations)
            .cloned()
            .collect();
        let missing = rust_analyzer_locations
            .difference(&rust_glancer_locations)
            .cloned()
            .collect();
        let extra = rust_glancer_locations
            .difference(&rust_analyzer_locations)
            .cloned()
            .collect();

        Self {
            rust_glancer_count: rust_glancer_locations.len(),
            rust_analyzer_count: rust_analyzer_locations.len(),
            matched,
            missing,
            extra,
            rust_glancer_unmapped_count: rust_glancer.unmapped_count(),
            rust_analyzer_unmapped_count: rust_analyzer.unmapped_count(),
            rust_glancer_unmapped: rust_glancer.unmapped_summaries(),
            rust_analyzer_unmapped: rust_analyzer.unmapped_summaries(),
        }
    }

    pub(crate) fn rust_glancer_count(&self) -> usize {
        self.rust_glancer_count
    }

    pub(crate) fn rust_analyzer_count(&self) -> usize {
        self.rust_analyzer_count
    }

    #[cfg(test)]
    pub(super) fn matched(&self) -> &[NormalizedLocation] {
        &self.matched
    }

    #[cfg(test)]
    pub(super) fn missing(&self) -> &[NormalizedLocation] {
        &self.missing
    }

    #[cfg(test)]
    pub(super) fn extra(&self) -> &[NormalizedLocation] {
        &self.extra
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

    pub(crate) fn rust_glancer_unmapped_count(&self) -> usize {
        self.rust_glancer_unmapped_count
    }

    pub(crate) fn rust_analyzer_unmapped_count(&self) -> usize {
        self.rust_analyzer_unmapped_count
    }

    pub(crate) fn rust_glancer_unmapped(&self) -> &[String] {
        &self.rust_glancer_unmapped
    }

    pub(crate) fn rust_analyzer_unmapped(&self) -> &[String] {
        &self.rust_analyzer_unmapped
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
pub(crate) struct LocationAggregate {
    query_count: usize,
    comparable_count: usize,
    non_comparable_count: usize,
    rust_glancer_locations: usize,
    rust_analyzer_locations: usize,
    matched_locations: usize,
    missing_locations: usize,
    extra_locations: usize,
    rust_glancer_unmapped_locations: usize,
    rust_analyzer_unmapped_locations: usize,
}

impl LocationAggregate {
    pub(super) fn record(&mut self, query: &QueryComparison) {
        self.query_count += 1;
        match query.result() {
            QueryComparisonResult::Locations(comparison) => {
                self.comparable_count += 1;
                self.rust_glancer_locations += comparison.rust_glancer_count();
                self.rust_analyzer_locations += comparison.rust_analyzer_count();
                self.matched_locations += comparison.matched_count();
                self.missing_locations += comparison.missing_count();
                self.extra_locations += comparison.extra_count();
                self.rust_glancer_unmapped_locations += comparison.rust_glancer_unmapped_count();
                self.rust_analyzer_unmapped_locations += comparison.rust_analyzer_unmapped_count();
            }
            QueryComparisonResult::NonComparable(_) => self.non_comparable_count += 1,
            QueryComparisonResult::Hover(_) => {}
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

    pub(crate) fn rust_glancer_locations(&self) -> usize {
        self.rust_glancer_locations
    }

    pub(crate) fn rust_analyzer_locations(&self) -> usize {
        self.rust_analyzer_locations
    }

    pub(crate) fn matched_locations(&self) -> usize {
        self.matched_locations
    }

    pub(crate) fn missing_locations(&self) -> usize {
        self.missing_locations
    }

    pub(crate) fn extra_locations(&self) -> usize {
        self.extra_locations
    }

    pub(crate) fn rust_glancer_unmapped_locations(&self) -> usize {
        self.rust_glancer_unmapped_locations
    }

    pub(crate) fn rust_analyzer_unmapped_locations(&self) -> usize {
        self.rust_analyzer_unmapped_locations
    }

    fn weighted_completeness(&self) -> Option<Ratio> {
        Ratio::new(self.matched_locations, self.rust_analyzer_locations)
    }

    fn precision_signal(&self) -> Option<Ratio> {
        Ratio::new(self.matched_locations, self.rust_glancer_locations)
    }

    pub(crate) fn weighted_completeness_percent(&self) -> Option<f64> {
        self.weighted_completeness().map(Ratio::percent)
    }

    pub(crate) fn precision_signal_percent(&self) -> Option<f64> {
        self.precision_signal().map(Ratio::percent)
    }
}
