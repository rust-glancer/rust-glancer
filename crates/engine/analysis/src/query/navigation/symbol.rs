//! Symbol-to-navigation resolution.

use anyhow::Context as _;
use rg_ir_model::identity::DeclarationRef;
use rg_ir_view::{IndexedViewDb, implementation::ImplementationView};

use crate::{
    model::{NavigationTarget, SymbolAt},
    query::navigation::target::NavigationTargetProjection,
    source_symbol::SourceSymbolResolver,
};

/// Resolves an already-selected analysis symbol into navigation destinations.
///
/// `SymbolAt` is cursor vocabulary, not a declaration identity. This resolver performs the
/// cross-IR lookups, path fallbacks, and body-resolution handling needed to turn one cursor symbol
/// into zero or more concrete targets.
pub(crate) struct SymbolResolver<'a, 'db>(&'a IndexedViewDb<'db>);

impl<'a, 'db> SymbolResolver<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self(db)
    }

    pub(crate) fn resolve_symbol(&self, symbol: SymbolAt) -> anyhow::Result<Vec<NavigationTarget>> {
        let source_symbols = SourceSymbolResolver::new(self.0);
        let projection = NavigationTargetProjection::new(self.0);
        // Calls carry more information than a declaration identity: `a.run()` and `b.run()`
        // can both name `Run::run` but use different impls. Definition navigation prefers the
        // one proven impl method; an inherited default or an unselected impl keeps the function
        // already resolved for the call. Other queries can keep using its declaration identity.
        if let Some(call) = source_symbols
            .call_for_symbol(&symbol)
            .context("resolve definition call context")?
            && let Some(facts) = call.facts()
        {
            let implementation = ImplementationView::new(self.0)
                .selected_call_implementation(&call)
                .context("find selected call implementation")?;
            let declaration = DeclarationRef::from(implementation.unwrap_or(facts.function()));
            if let Some(target) = projection
                .target_for_declaration(declaration)
                .context("project call definition")?
            {
                return Ok(vec![target]);
            }
        }

        // Non-call symbols and calls without a source destination use ordinary declarations.
        let declarations = source_symbols
            .declarations_for_symbol(symbol)
            .context("resolve definition declarations")?;
        projection
            .targets_for_declarations(declarations)
            .context("project definition targets")
    }
}
