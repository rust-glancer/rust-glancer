//! Shared comparison outcome helpers.

use crate::compare_lsp::normalization::NormalizedOutcome;

#[derive(Debug)]
pub(super) struct NonComparableComparison {
    rust_glancer: OutcomeStatus,
    rust_analyzer: OutcomeStatus,
}

impl NonComparableComparison {
    pub(super) fn new(rust_glancer: &NormalizedOutcome, rust_analyzer: &NormalizedOutcome) -> Self {
        Self {
            rust_glancer: OutcomeStatus::from_outcome(rust_glancer),
            rust_analyzer: OutcomeStatus::from_outcome(rust_analyzer),
        }
    }

    pub(super) fn summary(&self) -> String {
        format!(
            "non_comparable rust-glancer={}, rust-analyzer={}",
            self.rust_glancer.label(),
            self.rust_analyzer.label(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutcomeStatus {
    Locations,
    HoverPresent,
    HoverAbsent,
    MalformedSuccess,
    Error,
    Timeout,
    TransportFailure,
}

impl OutcomeStatus {
    fn from_outcome(outcome: &NormalizedOutcome) -> Self {
        match outcome {
            NormalizedOutcome::Locations(_) => Self::Locations,
            NormalizedOutcome::Hover { present: true } => Self::HoverPresent,
            NormalizedOutcome::Hover { present: false } => Self::HoverAbsent,
            NormalizedOutcome::MalformedSuccess { .. } => Self::MalformedSuccess,
            NormalizedOutcome::Error { .. } => Self::Error,
            NormalizedOutcome::Timeout => Self::Timeout,
            NormalizedOutcome::TransportFailure { .. } => Self::TransportFailure,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Locations => "locations",
            Self::HoverPresent => "hover_present",
            Self::HoverAbsent => "hover_absent",
            Self::MalformedSuccess => "malformed",
            Self::Error => "error",
            Self::Timeout => "timeout",
            Self::TransportFailure => "transport_failure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Ratio {
    numerator: usize,
    denominator: usize,
}

impl Ratio {
    pub(super) fn new(numerator: usize, denominator: usize) -> Option<Self> {
        (denominator > 0).then_some(Self {
            numerator,
            denominator,
        })
    }

    pub(super) fn percent(self) -> f64 {
        (self.numerator as f64 / self.denominator as f64) * 100.0
    }
}

pub(super) fn format_ratio(ratio: Option<Ratio>) -> String {
    ratio
        .map(|ratio| format!("{:.1}%", ratio.percent()))
        .unwrap_or_else(|| "n/a".to_string())
}
