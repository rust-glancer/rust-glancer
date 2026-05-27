//! Symbol-to-navigation resolution.

use crate::{
    api::{
        Analysis, query::navigation::target::NavigationTargetProjection,
        source_symbol::SourceSymbolResolver,
    },
    model::{NavigationTarget, SymbolAt},
};

/// Resolves an already-selected analysis symbol into navigation destinations.
///
/// `SymbolAt` is cursor vocabulary, not a declaration identity. This resolver performs the
/// cross-IR lookups, path fallbacks, and body-resolution handling needed to turn one cursor symbol
/// into zero or more concrete targets.
pub(crate) struct SymbolResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> SymbolResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn resolve_symbol(&self, symbol: SymbolAt) -> anyhow::Result<Vec<NavigationTarget>> {
        let declarations = SourceSymbolResolver::new(self.0).declarations_for_symbol(symbol)?;
        NavigationTargetProjection::new(self.0).targets_for_declarations(declarations)
    }
}
