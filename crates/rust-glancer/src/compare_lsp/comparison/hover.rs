//! Hover-result comparison and aggregation.

use super::metrics::{AggregateSummaryMetrics, HoverAggregateMetrics, HoverComparisonMetrics};
use crate::compare_lsp::comparison::{QueryComparison, QueryComparisonResult};

#[derive(Debug)]
pub(crate) struct HoverComparison {
    rust_glancer_present: bool,
    rust_analyzer_present: bool,
}

impl HoverComparison {
    pub(super) fn new(rust_glancer_present: bool, rust_analyzer_present: bool) -> Self {
        Self {
            rust_glancer_present,
            rust_analyzer_present,
        }
    }

    pub(crate) fn metrics(&self) -> HoverComparisonMetrics {
        HoverComparisonMetrics {
            rust_glancer_present: self.rust_glancer_present,
            rust_analyzer_present: self.rust_analyzer_present,
            agreement: self.rust_glancer_present == self.rust_analyzer_present,
            rust_glancer_missing: !self.rust_glancer_present && self.rust_analyzer_present,
            rust_glancer_extra_present: self.rust_glancer_present && !self.rust_analyzer_present,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct HoverAggregate {
    query_count: usize,
    comparable_count: usize,
    agreement_count: usize,
    rust_glancer_missing_count: usize,
    rust_glancer_extra_present_count: usize,
    non_comparable_count: usize,
}

impl HoverAggregate {
    pub(super) fn record(&mut self, query: &QueryComparison) {
        self.query_count += 1;
        match query.result() {
            QueryComparisonResult::Hover(comparison) => {
                let metrics = comparison.metrics();
                self.comparable_count += 1;
                if metrics.agreement {
                    self.agreement_count += 1;
                }
                if metrics.rust_glancer_missing {
                    self.rust_glancer_missing_count += 1;
                }
                if metrics.rust_glancer_extra_present {
                    self.rust_glancer_extra_present_count += 1;
                }
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

    pub(crate) fn metrics(&self) -> HoverAggregateMetrics {
        HoverAggregateMetrics {
            agreement_count: self.agreement_count,
            rust_glancer_missing_count: self.rust_glancer_missing_count,
            rust_glancer_extra_present_count: self.rust_glancer_extra_present_count,
        }
    }
}
