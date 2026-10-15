//! Location-result comparison and aggregation.

use std::collections::BTreeSet;

use super::metrics::{
    AggregateSummaryMetrics, MappedSetAggregateMetrics, MappedSetComparisonMetrics,
    SetComparisonMetrics,
};
use crate::compare_lsp::{
    comparison::{QueryComparison, QueryComparisonResult},
    normalization::{NormalizedLocation, NormalizedLocationSet},
    query::QueryKind,
};

#[derive(Debug)]
pub(crate) struct LocationComparison {
    rust_glancer_count: usize,
    rust_analyzer_count: usize,
    matched: Vec<NormalizedLocation>,
    compatible: Vec<(NormalizedLocation, NormalizedLocation)>,
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
        query_kind: QueryKind,
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
        let mut matched = rust_glancer_locations
            .intersection(&rust_analyzer_locations)
            .cloned()
            .collect::<Vec<_>>();
        let mut missing = rust_analyzer_locations
            .difference(&rust_glancer_locations)
            .cloned()
            .collect::<Vec<_>>();
        let unmatched_extra = rust_glancer_locations
            .difference(&rust_analyzer_locations)
            .cloned()
            .collect::<Vec<_>>();

        // Implementation navigation lands at the range's start. Selecting an entire macro
        // invocation or just `make_type!` opens the same destination. Match those one-to-one
        // after exact matches; reference occurrences still require their complete token ranges.
        let mut compatible = Vec::new();
        let mut extra = Vec::new();
        for location in unmatched_extra {
            if matches!(query_kind, QueryKind::Implementation)
                && let Some(index) = missing
                    .iter()
                    .position(|reference| location.has_same_start_as(reference))
            {
                let reference = missing.remove(index);
                matched.push(location.clone());
                compatible.push((location, reference));
            } else {
                extra.push(location);
            }
        }
        matched.sort();

        Self {
            rust_glancer_count: rust_glancer_locations.len(),
            rust_analyzer_count: rust_analyzer_locations.len(),
            matched,
            compatible,
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
            set: SetComparisonMetrics::new_with_compatible_matches(
                self.rust_glancer_count,
                self.rust_analyzer_count,
                self.matched.len() - self.compatible.len(),
                self.compatible.len(),
                self.missing.len(),
                self.extra.len(),
            ),
            rust_glancer_unmapped_count: self.rust_glancer_unmapped_count,
            rust_analyzer_unmapped_count: self.rust_analyzer_unmapped_count,
            rust_glancer_unmapped: self.rust_glancer_unmapped.clone(),
            rust_analyzer_unmapped: self.rust_analyzer_unmapped.clone(),
        }
    }

    #[cfg(test)]
    pub(super) fn matched(&self) -> &[NormalizedLocation] {
        &self.matched
    }

    pub(super) fn missing(&self) -> &[NormalizedLocation] {
        &self.missing
    }

    pub(super) fn compatible(&self) -> &[(NormalizedLocation, NormalizedLocation)] {
        &self.compatible
    }

    pub(super) fn extra(&self) -> &[NormalizedLocation] {
        &self.extra
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
    compatible_locations: usize,
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
                self.rust_glancer_locations += comparison.rust_glancer_count;
                self.rust_analyzer_locations += comparison.rust_analyzer_count;
                self.matched_locations += comparison.matched.len();
                self.compatible_locations += comparison.compatible.len();
                self.missing_locations += comparison.missing.len();
                self.extra_locations += comparison.extra.len();
                self.rust_glancer_unmapped_locations += comparison.rust_glancer_unmapped_count;
                self.rust_analyzer_unmapped_locations += comparison.rust_analyzer_unmapped_count;
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
            set: SetComparisonMetrics::new_with_compatible_matches(
                self.rust_glancer_locations,
                self.rust_analyzer_locations,
                self.matched_locations - self.compatible_locations,
                self.compatible_locations,
                self.missing_locations,
                self.extra_locations,
            ),
            rust_glancer_unmapped_count: self.rust_glancer_unmapped_locations,
            rust_analyzer_unmapped_count: self.rust_analyzer_unmapped_locations,
        }
    }
}
