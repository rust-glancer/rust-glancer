//! Symbol-result comparison and aggregation.

use std::collections::BTreeSet;

use super::metrics::{
    AggregateSummaryMetrics, MappedSetAggregateMetrics, MappedSetComparisonMetrics,
    SetComparisonMetrics,
};
use crate::compare_lsp::{
    comparison::{QueryComparison, QueryComparisonResult},
    normalization::{NormalizedSymbol, NormalizedSymbolSet},
};

#[derive(Debug)]
pub(crate) struct SymbolComparison {
    rust_glancer_count: usize,
    rust_analyzer_count: usize,
    matched: Vec<NormalizedSymbol>,
    compatible: Vec<(NormalizedSymbol, NormalizedSymbol)>,
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

        let mut matched = rust_glancer_symbols
            .intersection(&rust_analyzer_symbols)
            .cloned()
            .collect::<Vec<_>>();
        let mut missing = rust_analyzer_symbols
            .difference(&rust_glancer_symbols)
            .cloned()
            .collect::<Vec<_>>();
        let unmatched_extra = rust_glancer_symbols
            .difference(&rust_analyzer_symbols)
            .cloned()
            .collect::<Vec<_>>();

        let mut compatible = Vec::new();
        let mut extra = Vec::new();
        for rust_glancer_symbol in unmatched_extra {
            let Some(reference_index) = missing
                .iter()
                .position(|reference| rust_glancer_symbol.is_no_worse_match_for(reference))
            else {
                extra.push(rust_glancer_symbol);
                continue;
            };

            let rust_analyzer_symbol = missing.remove(reference_index);
            matched.push(rust_glancer_symbol.clone());
            compatible.push((rust_glancer_symbol, rust_analyzer_symbol));
        }
        matched.sort();

        Self {
            rust_glancer_count: rust_glancer_symbols.len(),
            rust_analyzer_count: rust_analyzer_symbols.len(),
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

    pub(super) fn missing(&self) -> &[NormalizedSymbol] {
        &self.missing
    }

    pub(super) fn extra(&self) -> &[NormalizedSymbol] {
        &self.extra
    }

    pub(super) fn compatible(&self) -> &[(NormalizedSymbol, NormalizedSymbol)] {
        &self.compatible
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
    compatible_symbols: usize,
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
                self.rust_glancer_symbols += comparison.rust_glancer_count;
                self.rust_analyzer_symbols += comparison.rust_analyzer_count;
                self.matched_symbols += comparison.matched.len();
                self.compatible_symbols += comparison.compatible.len();
                self.missing_symbols += comparison.missing.len();
                self.extra_symbols += comparison.extra.len();
                self.rust_glancer_unmapped_symbols += comparison.rust_glancer_unmapped_count;
                self.rust_analyzer_unmapped_symbols += comparison.rust_analyzer_unmapped_count;
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
                self.rust_glancer_symbols,
                self.rust_analyzer_symbols,
                self.matched_symbols - self.compatible_symbols,
                self.compatible_symbols,
                self.missing_symbols,
                self.extra_symbols,
            ),
            rust_glancer_unmapped_count: self.rust_glancer_unmapped_symbols,
            rust_analyzer_unmapped_count: self.rust_analyzer_unmapped_symbols,
        }
    }
}

#[cfg(test)]
mod tests {
    use ls_types::SymbolKind;

    use crate::compare_lsp::normalization::{
        NormalizedRange, NormalizedSymbol, NormalizedSymbolSet,
    };

    use super::SymbolComparison;

    #[test]
    fn accepts_method_as_a_more_specific_function_classification() {
        let range = NormalizedRange::test_new(8, 11, 8, 15);
        let rust_glancer = symbols(vec![symbol(SymbolKind::METHOD, range)]);
        let rust_analyzer = symbols(vec![symbol(SymbolKind::FUNCTION, range)]);

        let comparison = SymbolComparison::new(&rust_glancer, &rust_analyzer);
        let metrics = comparison.metrics().set;

        assert_eq!(metrics.matched_count, 1);
        assert_eq!(metrics.compatible_count, 1);
        assert_eq!(metrics.missing_count, 0);
        assert_eq!(metrics.extra_count, 0);
    }

    #[test]
    fn rejects_function_when_the_reference_knows_it_is_a_method() {
        let range = NormalizedRange::test_new(8, 11, 8, 15);
        let rust_glancer = symbols(vec![symbol(SymbolKind::FUNCTION, range)]);
        let rust_analyzer = symbols(vec![symbol(SymbolKind::METHOD, range)]);

        let comparison = SymbolComparison::new(&rust_glancer, &rust_analyzer);
        let metrics = comparison.metrics().set;

        assert_eq!(metrics.matched_count, 0);
        assert_eq!(metrics.compatible_count, 0);
        assert_eq!(metrics.missing_count, 1);
        assert_eq!(metrics.extra_count, 1);
    }

    #[test]
    fn keeps_broader_rust_glancer_symbol_ranges_as_divergences() {
        let focused = NormalizedRange::test_new(8, 5, 8, 15);
        let whole_impl = NormalizedRange::test_new(8, 0, 14, 1);
        let rust_glancer = symbols(vec![symbol(SymbolKind::OBJECT, whole_impl)]);
        let rust_analyzer = symbols(vec![symbol(SymbolKind::OBJECT, focused)]);

        let comparison = SymbolComparison::new(&rust_glancer, &rust_analyzer);
        let metrics = comparison.metrics().set;

        assert_eq!(metrics.matched_count, 0);
        assert_eq!(metrics.compatible_count, 0);
        assert_eq!(metrics.missing_count, 1);
        assert_eq!(metrics.extra_count, 1);
    }

    #[test]
    fn keeps_unrelated_lossy_type_kinds_as_divergences() {
        let range = NormalizedRange::test_new(8, 5, 8, 14);
        let rust_glancer = symbols(vec![symbol(SymbolKind::CLASS, range)]);
        let rust_analyzer = symbols(vec![symbol(SymbolKind::TYPE_PARAMETER, range)]);

        let comparison = SymbolComparison::new(&rust_glancer, &rust_analyzer);
        let metrics = comparison.metrics().set;

        assert_eq!(metrics.matched_count, 0);
        assert_eq!(metrics.compatible_count, 0);
        assert_eq!(metrics.missing_count, 1);
        assert_eq!(metrics.extra_count, 1);
    }

    fn symbols(symbols: Vec<NormalizedSymbol>) -> NormalizedSymbolSet {
        NormalizedSymbolSet::test_from_symbols(symbols)
    }

    fn symbol(kind: SymbolKind, range: NormalizedRange) -> NormalizedSymbol {
        NormalizedSymbol::test_new("save", kind, Some("src/lib.rs"), Some(range))
    }
}
