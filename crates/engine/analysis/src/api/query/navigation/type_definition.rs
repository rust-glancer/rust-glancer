//! Goto-type-definition query flow.

use rg_def_map::TargetRef;
use rg_parse::FileId;

use super::target;
use crate::{
    api::{Analysis, query::type_at::TypeResolver},
    model::NavigationTarget,
};

/// Implements goto-type-definition through the shared type query.
///
/// This resolver asks `type_at` for the best-effort body type at the cursor and then navigates to
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
        let Some(ty) = TypeResolver::new(self.0).type_at(target, file_id, offset)? else {
            return Ok(Vec::new());
        };

        target::NavigationTargetResolver::new(self.0).navigation_targets_for_body_ty(&ty)
    }
}
