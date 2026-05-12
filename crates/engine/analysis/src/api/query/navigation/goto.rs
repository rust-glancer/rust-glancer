//! Goto-definition query flow.

use rg_def_map::TargetRef;
use rg_parse::FileId;

use super::SymbolResolver;
use crate::{api::Analysis, model::NavigationTarget};

/// Implements goto-definition as symbol selection followed by symbol resolution.
///
/// The cursor lookup and the target lookup are deliberately separate so callers can also resolve a
/// previously captured `SymbolAt` without re-reading the source position.
pub(crate) struct GotoResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> GotoResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn goto_definition(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        let Some(symbol) = self.0.symbol_at_for_query(target, file_id, offset)? else {
            return Ok(Vec::new());
        };

        SymbolResolver::new(self.0).resolve_symbol(symbol)
    }
}
