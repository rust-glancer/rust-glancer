//! Goto-type-definition query flow.

use rg_ir_model::{CrateRef, FileId};

use super::target::NavigationTargetProjection;
use crate::{Analysis, model::NavigationTarget, source_symbol::SourceSymbolResolver};

/// Implements goto-type-definition through the shared type query.
///
/// Resolve the types of the selected source occurrences, then navigate to their declarations,
/// such as `Service` in `&dyn Service`.
pub(crate) struct TypeDefinitionResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> TypeDefinitionResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn goto_type_definition(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        // A repeated macro argument can name methods with different return types. Gather each
        // expanded use's type declarations before the projection removes duplicate targets.
        let resolver = SourceSymbolResolver::new(self.0.view_db());
        let mut declarations = Vec::new();
        for source in self
            .0
            .source_symbols_at_for_query(crate_ref, file_id, offset)?
        {
            if let Some(ty) = resolver.ty_for_symbol(source.into_symbol())? {
                declarations.extend(ty.contained_type_declarations());
            }
        }
        NavigationTargetProjection::new(self.0.view_db()).targets_for_declarations(declarations)
    }
}
