//! Shared comparison outcome helpers.

use crate::compare_lsp::normalization::NormalizedOutcome;

#[derive(Debug)]
pub(crate) struct NonComparableComparison {
    rust_glancer: OutcomeStatus,
    rust_analyzer: OutcomeStatus,
    rust_glancer_detail: Option<String>,
    rust_analyzer_detail: Option<String>,
}

impl NonComparableComparison {
    pub(super) fn new(rust_glancer: &NormalizedOutcome, rust_analyzer: &NormalizedOutcome) -> Self {
        Self {
            rust_glancer: OutcomeStatus::from_outcome(rust_glancer),
            rust_analyzer: OutcomeStatus::from_outcome(rust_analyzer),
            rust_glancer_detail: outcome_detail(rust_glancer),
            rust_analyzer_detail: outcome_detail(rust_analyzer),
        }
    }

    pub(crate) fn rust_glancer_status(&self) -> OutcomeStatus {
        self.rust_glancer
    }

    pub(crate) fn rust_analyzer_status(&self) -> OutcomeStatus {
        self.rust_analyzer
    }

    pub(crate) fn rust_glancer_detail(&self) -> Option<&str> {
        self.rust_glancer_detail.as_deref()
    }

    pub(crate) fn rust_analyzer_detail(&self) -> Option<&str> {
        self.rust_analyzer_detail.as_deref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutcomeStatus {
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

    pub(crate) fn label(self) -> &'static str {
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

fn outcome_detail(outcome: &NormalizedOutcome) -> Option<String> {
    match outcome {
        NormalizedOutcome::MalformedSuccess { message } => Some(message.clone()),
        NormalizedOutcome::Error { code, message } => Some(format!("{code}: {message}")),
        NormalizedOutcome::TransportFailure { message } => Some(message.clone()),
        NormalizedOutcome::Locations(_)
        | NormalizedOutcome::Hover { .. }
        | NormalizedOutcome::Timeout => None,
    }
}
