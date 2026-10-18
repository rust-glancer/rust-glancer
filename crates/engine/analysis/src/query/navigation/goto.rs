//! Goto-definition query flow.

use anyhow::Context as _;
use rg_ir_model::{CrateRef, FileId};
use rg_std::UniqueVec;

use super::{NavigationTargetProjection, SymbolResolver};
use crate::{Analysis, documentation::SourceDocumentationQuery, model::NavigationTarget};

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
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        if let Some(link) = SourceDocumentationQuery::new(self.0)
            .link_at(crate_ref, file_id, offset)
            .context("find documentation navigation link")?
        {
            return Ok(NavigationTargetProjection::new(self.0.view_db())
                .target_for_declaration(link.declaration)
                .context("project documentation target")?
                .into_iter()
                .collect());
        }
        // One written macro argument may refer to several declarations after expansion.
        let mut targets = UniqueVec::new();
        let resolver = SymbolResolver::new(self.0.view_db());
        for source in self
            .0
            .source_symbols_at_for_query(crate_ref, file_id, offset)?
        {
            targets.extend(resolver.resolve_symbol(source.into_symbol())?);
        }
        Ok(targets.into_vec())
    }
}
