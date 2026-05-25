//! Best-effort type queries over analysis symbols.
//!
//! The public query returns Body IR types because they can describe both semantic items and
//! body-local declarations.

use rg_body_ir::BodyTy;
use rg_def_map::TargetRef;
use rg_parse::FileId;

use crate::api::{Analysis, view::ty::TyView};

pub(crate) struct TypeResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> TypeResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn type_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<BodyTy>> {
        let Some(symbol) = self.0.symbol_at_for_query(target, file_id, offset)? else {
            return Ok(None);
        };

        TyView::new(self.0).ty_for_symbol(symbol)
    }
}
