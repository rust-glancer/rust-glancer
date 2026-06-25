//! Hover-result comparison and aggregation.

use crate::compare_lsp::comparison::{QueryComparison, QueryComparisonResult};

#[derive(Debug)]
pub(super) struct HoverComparison {
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

    fn agrees(&self) -> bool {
        self.rust_glancer_present == self.rust_analyzer_present
    }

    fn rust_glancer_missing(&self) -> bool {
        !self.rust_glancer_present && self.rust_analyzer_present
    }

    fn rust_glancer_extra_present(&self) -> bool {
        self.rust_glancer_present && !self.rust_analyzer_present
    }

    pub(super) fn summary(&self) -> String {
        format!(
            "hover rust-glancer={}, rust-analyzer={}, agreement={}",
            presence(self.rust_glancer_present),
            presence(self.rust_analyzer_present),
            self.agrees(),
        )
    }
}

#[derive(Debug, Default)]
pub(super) struct HoverAggregate {
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
            QueryComparisonResult::Locations(_) => {}
        }
    }

    pub(super) fn query_count(&self) -> usize {
        self.query_count
    }

    #[cfg(test)]
    pub(super) fn comparable_count(&self) -> usize {
        self.comparable_count
    }

    #[cfg(test)]
    pub(super) fn agreement_count(&self) -> usize {
        self.agreement_count
    }

    #[cfg(test)]
    pub(super) fn rust_glancer_missing_count(&self) -> usize {
        self.rust_glancer_missing_count
    }

    #[cfg(test)]
    pub(super) fn rust_glancer_extra_present_count(&self) -> usize {
        self.rust_glancer_extra_present_count
    }

    #[cfg(test)]
    pub(super) fn non_comparable_count(&self) -> usize {
        self.non_comparable_count
    }

    pub(super) fn summary(&self) -> String {
        format!(
            "comparable={}/{}, agreements={}, rust_glancer_missing={}, \
             rust_glancer_extra_present={}, non_comparable={}",
            self.comparable_count,
            self.query_count,
            self.agreement_count,
            self.rust_glancer_missing_count,
            self.rust_glancer_extra_present_count,
            self.non_comparable_count,
        )
    }
}

fn presence(present: bool) -> &'static str {
    if present { "present" } else { "absent" }
}
