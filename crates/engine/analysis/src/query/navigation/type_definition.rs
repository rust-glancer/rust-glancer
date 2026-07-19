//! Goto-type-definition query flow.

use rg_ir_model::{CrateRef, identity::DeclarationRef};
use rg_parse::FileId;

use super::target::NavigationTargetProjection;
use crate::{Analysis, model::NavigationTarget, source_symbol::SourceSymbolResolver};

/// Implements goto-type-definition through the shared type query.
///
/// This resolver asks `type_at` for the best-effort type at the cursor and then navigates to
/// the nominal declarations represented by that type.
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
        let Some(symbol) = self.0.symbol_at_for_query(crate_ref, file_id, offset)? else {
            return Ok(Vec::new());
        };

        let Some(ty) = SourceSymbolResolver::new(self.0.view_db()).ty_for_symbol(symbol)? else {
            return Ok(Vec::new());
        };

        NavigationTargetProjection::new(self.0.view_db())
            .targets_for_declarations(ty.nominal_type_defs().map(DeclarationRef::from))
    }
}
