//! Goto-type-definition query flow.

use rg_ir_model::TargetRef;
use rg_parse::FileId;

use super::target::NavigationTargetProjection;
use crate::{
    api::{Analysis, view::ty::TyView},
    model::NavigationTarget,
};

/// Implements goto-type-definition through the shared type query.
///
/// This resolver asks `type_at` for the best-effort indexed type at the cursor and then navigates to
/// the nominal declarations represented by that type.
pub(crate) struct TypeDefinitionResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> TypeDefinitionResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn goto_type_definition(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        let Some(symbol) = self.0.symbol_at_for_query(target, file_id, offset)? else {
            return Ok(Vec::new());
        };

        let ty_view = TyView::new(self.0);
        let Some(ty) = self.0.ty_for_source_symbol(symbol)? else {
            return Ok(Vec::new());
        };

        let declarations = ty_view.declarations_for_ty(&ty);
        NavigationTargetProjection::new(self.0).targets_for_declarations(declarations)
    }
}
