//! Symbol-result comparison and aggregation.

use std::collections::BTreeSet;

use crate::compare_lsp::{
    comparison::{QueryComparison, QueryComparisonResult, outcome::Ratio},
    normalization::{NormalizedSymbol, NormalizedSymbolSet},
};

#[derive(Debug)]
pub(crate) struct SymbolComparison {
    rust_glancer_count: usize,
    rust_analyzer_count: usize,
    matched: Vec<NormalizedSymbol>,
    missing: Vec<NormalizedSymbol>,
    extra: Vec<NormalizedSymbol>,
    rust_glancer_unmapped_count: usize,
    rust_analyzer_unmapped_count: usize,
    rust_glancer_unmapped: Vec<String>,
    rust_analyzer_unmapped: Vec<String>,
}

impl SymbolComparison {
    pub(super) fn new(
        rust_glancer: &NormalizedSymbolSet,
        rust_analyzer: &NormalizedSymbolSet,
    ) -> Self {
        let rust_glancer_symbols = rust_glancer
            .symbols()
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let rust_analyzer_symbols = rust_analyzer
            .symbols()
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();

        let matched = rust_glancer_symbols
            .intersection(&rust_analyzer_symbols)
            .cloned()
            .collect();
        let missing = rust_analyzer_symbols
            .difference(&rust_glancer_symbols)
            .cloned()
            .collect();
        let extra = rust_glancer_symbols
            .difference(&rust_analyzer_symbols)
            .cloned()
            .collect();

        Self {
            rust_glancer_count: rust_glancer_symbols.len(),
            rust_analyzer_count: rust_analyzer_symbols.len(),
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
pub(crate) struct SymbolAggregate {
    query_count: usize,
    comparable_count: usize,
    non_comparable_count: usize,
    rust_glancer_symbols: usize,
    rust_analyzer_symbols: usize,
    matched_symbols: usize,
    missing_symbols: usize,
    extra_symbols: usize,
    rust_glancer_unmapped_symbols: usize,
    rust_analyzer_unmapped_symbols: usize,
}

impl SymbolAggregate {
    pub(super) fn record(&mut self, query: &QueryComparison) {
        self.query_count += 1;
        match query.result() {
            QueryComparisonResult::Symbols(comparison) => {
                self.comparable_count += 1;
                self.rust_glancer_symbols += comparison.rust_glancer_count();
                self.rust_analyzer_symbols += comparison.rust_analyzer_count();
                self.matched_symbols += comparison.matched_count();
                self.missing_symbols += comparison.missing_count();
                self.extra_symbols += comparison.extra_count();
                self.rust_glancer_unmapped_symbols += comparison.rust_glancer_unmapped_count();
                self.rust_analyzer_unmapped_symbols += comparison.rust_analyzer_unmapped_count();
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

    pub(crate) fn rust_glancer_symbols(&self) -> usize {
        self.rust_glancer_symbols
    }

    pub(crate) fn rust_analyzer_symbols(&self) -> usize {
        self.rust_analyzer_symbols
    }

    pub(crate) fn matched_symbols(&self) -> usize {
        self.matched_symbols
    }

    pub(crate) fn missing_symbols(&self) -> usize {
        self.missing_symbols
    }

    pub(crate) fn extra_symbols(&self) -> usize {
        self.extra_symbols
    }

    pub(crate) fn rust_glancer_unmapped_symbols(&self) -> usize {
        self.rust_glancer_unmapped_symbols
    }

    pub(crate) fn rust_analyzer_unmapped_symbols(&self) -> usize {
        self.rust_analyzer_unmapped_symbols
    }

    fn weighted_completeness(&self) -> Option<Ratio> {
        Ratio::new(self.matched_symbols, self.rust_analyzer_symbols)
    }

    fn precision_signal(&self) -> Option<Ratio> {
        Ratio::new(self.matched_symbols, self.rust_glancer_symbols)
    }

    pub(crate) fn weighted_completeness_percent(&self) -> Option<f64> {
        self.weighted_completeness().map(Ratio::percent)
    }

    pub(crate) fn precision_signal_percent(&self) -> Option<f64> {
        self.precision_signal().map(Ratio::percent)
    }
}
