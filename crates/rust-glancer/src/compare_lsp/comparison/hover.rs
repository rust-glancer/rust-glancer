//! Hover-result comparison and aggregation.

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

    pub(crate) fn rust_glancer_present(&self) -> bool {
        self.rust_glancer_present
    }

    pub(crate) fn rust_analyzer_present(&self) -> bool {
        self.rust_analyzer_present
    }

    pub(crate) fn agrees(&self) -> bool {
        self.rust_glancer_present == self.rust_analyzer_present
    }

    pub(crate) fn rust_glancer_missing(&self) -> bool {
        !self.rust_glancer_present && self.rust_analyzer_present
    }

    pub(crate) fn rust_glancer_extra_present(&self) -> bool {
        self.rust_glancer_present && !self.rust_analyzer_present
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
                self.comparable_count += 1;
                if comparison.agrees() {
                    self.agreement_count += 1;
                }
                if comparison.rust_glancer_missing() {
                    self.rust_glancer_missing_count += 1;
                }
                if comparison.rust_glancer_extra_present() {
                    self.rust_glancer_extra_present_count += 1;
                }
            }
            QueryComparisonResult::NonComparable(_) => self.non_comparable_count += 1,
            QueryComparisonResult::Locations(_) | QueryComparisonResult::Ranges(_) => {}
        }
    }

    pub(crate) fn query_count(&self) -> usize {
        self.query_count
    }

    pub(crate) fn comparable_count(&self) -> usize {
        self.comparable_count
    }

    pub(crate) fn agreement_count(&self) -> usize {
        self.agreement_count
    }

    pub(crate) fn rust_glancer_missing_count(&self) -> usize {
        self.rust_glancer_missing_count
    }

    pub(crate) fn rust_glancer_extra_present_count(&self) -> usize {
        self.rust_glancer_extra_present_count
    }

    pub(crate) fn non_comparable_count(&self) -> usize {
        self.non_comparable_count
    }
}
