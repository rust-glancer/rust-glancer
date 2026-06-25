//! Inlay-hint comparison and aggregation.

use std::collections::BTreeSet;

use super::metrics::{AggregateSummaryMetrics, SetComparisonMetrics};
use crate::compare_lsp::{
    comparison::{QueryComparison, QueryComparisonResult},
    normalization::{NormalizedInlayHint, NormalizedInlayHintSet},
};

#[derive(Debug)]
pub(crate) struct InlayHintComparison {
    rust_glancer_count: usize,
    rust_analyzer_count: usize,
    matched: Vec<NormalizedInlayHint>,
    missing: Vec<NormalizedInlayHint>,
    extra: Vec<NormalizedInlayHint>,
}

impl InlayHintComparison {
    pub(super) fn new(
        rust_glancer: &NormalizedInlayHintSet,
        rust_analyzer: &NormalizedInlayHintSet,
    ) -> Self {
        let rust_glancer_hints = rust_glancer
            .hints()
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let rust_analyzer_hints = rust_analyzer
            .hints()
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();

        let matched = rust_glancer_hints
            .intersection(&rust_analyzer_hints)
            .cloned()
            .collect();
        let missing = rust_analyzer_hints
            .difference(&rust_glancer_hints)
            .cloned()
            .collect();
        let extra = rust_glancer_hints
            .difference(&rust_analyzer_hints)
            .cloned()
            .collect();

        Self {
            rust_glancer_count: rust_glancer_hints.len(),
            rust_analyzer_count: rust_analyzer_hints.len(),
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
pub(crate) struct InlayHintAggregate {
    query_count: usize,
    comparable_count: usize,
    non_comparable_count: usize,
    rust_glancer_hints: usize,
    rust_analyzer_hints: usize,
    matched_hints: usize,
    missing_hints: usize,
    extra_hints: usize,
}

impl InlayHintAggregate {
    pub(super) fn record(&mut self, query: &QueryComparison) {
        self.query_count += 1;
        match query.result() {
            QueryComparisonResult::InlayHints(comparison) => {
                self.comparable_count += 1;
                self.rust_glancer_hints += comparison.rust_glancer_count;
                self.rust_analyzer_hints += comparison.rust_analyzer_count;
                self.matched_hints += comparison.matched.len();
                self.missing_hints += comparison.missing.len();
                self.extra_hints += comparison.extra.len();
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
            self.rust_glancer_hints,
            self.rust_analyzer_hints,
            self.matched_hints,
            self.missing_hints,
            self.extra_hints,
        )
    }
}
