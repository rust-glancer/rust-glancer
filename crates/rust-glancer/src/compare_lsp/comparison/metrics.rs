//! Compact metric snapshots exported by the comparison layer.

use super::outcome::{OutcomeStatus, Ratio};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SetComparisonMetrics {
    pub(crate) rust_glancer_count: usize,
    pub(crate) rust_analyzer_count: usize,
    pub(crate) matched_count: usize,
    pub(crate) missing_count: usize,
    pub(crate) extra_count: usize,
    pub(crate) match_score_percent: f64,
    pub(crate) recall_percent: Option<f64>,
    pub(crate) precision_percent: Option<f64>,
}

impl SetComparisonMetrics {
    pub(super) fn new(
        rust_glancer_count: usize,
        rust_analyzer_count: usize,
        matched_count: usize,
        missing_count: usize,
        extra_count: usize,
    ) -> Self {
        Self {
            rust_glancer_count,
            rust_analyzer_count,
            matched_count,
            missing_count,
            extra_count,
            match_score_percent: Self::match_score_percent(
                rust_glancer_count,
                rust_analyzer_count,
                matched_count,
            ),
            recall_percent: Ratio::new(matched_count, rust_analyzer_count).map(Ratio::percent),
            precision_percent: Ratio::new(matched_count, rust_glancer_count).map(Ratio::percent),
        }
    }

    fn match_score_percent(
        rust_glancer_count: usize,
        rust_analyzer_count: usize,
        matched_count: usize,
    ) -> f64 {
        let total_count = rust_glancer_count + rust_analyzer_count;
        if total_count == 0 {
            100.0
        } else {
            (2.0 * matched_count as f64 / total_count as f64) * 100.0
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MappedSetComparisonMetrics {
    pub(crate) set: SetComparisonMetrics,
    pub(crate) rust_glancer_unmapped_count: usize,
    pub(crate) rust_analyzer_unmapped_count: usize,
    pub(crate) rust_glancer_unmapped: Vec<String>,
    pub(crate) rust_analyzer_unmapped: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MappedSetAggregateMetrics {
    pub(crate) set: SetComparisonMetrics,
    pub(crate) rust_glancer_unmapped_count: usize,
    pub(crate) rust_analyzer_unmapped_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AggregateSummaryMetrics {
    pub(crate) query_count: usize,
    pub(crate) comparable_count: usize,
    pub(crate) non_comparable_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HoverComparisonMetrics {
    pub(crate) rust_glancer_present: bool,
    pub(crate) rust_analyzer_present: bool,
    pub(crate) agreement: bool,
    pub(crate) rust_glancer_missing: bool,
    pub(crate) rust_glancer_extra_present: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HoverAggregateMetrics {
    pub(crate) agreement_count: usize,
    pub(crate) rust_glancer_missing_count: usize,
    pub(crate) rust_glancer_extra_present_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NonComparableMetrics {
    pub(crate) rust_glancer_status: OutcomeStatus,
    pub(crate) rust_analyzer_status: OutcomeStatus,
    pub(crate) rust_glancer_detail: Option<String>,
    pub(crate) rust_analyzer_detail: Option<String>,
}
