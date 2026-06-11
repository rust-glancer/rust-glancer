//! Body-aware resolution adapter for view projections.
//!
//! Body resolution still lives in `rg_body_ir`, but view modules should not each know how to
//! construct its context. This adapter is the single `ir-view` entry point for body-local path and
//! member facts.

use rg_body_ir::BodyResolutionContext;
use rg_ir_model::{BodyRef, Path, ScopeId, TypePathResolution, identity::DeclarationRef};
use rg_ty::{MemberMethodCandidateRef, Ty};

use crate::IndexedViewDb;

pub(crate) struct BodyResolutionView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> BodyResolutionView<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub(crate) fn type_path_resolution(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Option<TypePathResolution>> {
        let Some(body) = self.db.body_ir.body_data(body_ref)? else {
            return Ok(None);
        };

        Ok(Some(
            BodyResolutionContext::new(self.db, self.db, body_ref, body)
                .type_path_query()
                .resolve_in_scope(scope, path)?,
        ))
    }

    pub(crate) fn nonlocal_value_path_declarations(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let Some(body) = self.db.body_ir.body_data(body_ref)? else {
            return Ok(Vec::new());
        };

        Ok(BodyResolutionContext::new(self.db, self.db, body_ref, body)
            .value_paths()
            .resolve_nonlocal_path_declarations(scope, path)?)
    }

    pub(crate) fn nonlocal_value_path_ty(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Ty> {
        let Some(body) = self.db.body_ir.body_data(body_ref)? else {
            return Ok(Ty::Unknown);
        };

        Ok(BodyResolutionContext::new(self.db, self.db, body_ref, body)
            .value_paths()
            .resolve_nonlocal_path_ty(scope, path)?)
    }

    pub(crate) fn receiver_method_candidates_for_ty(
        &self,
        body_ref: BodyRef,
        ty: &Ty,
    ) -> anyhow::Result<Option<Vec<MemberMethodCandidateRef>>> {
        let Some(body) = self.db.body_ir.body_data(body_ref)? else {
            return Ok(None);
        };

        Ok(Some(
            BodyResolutionContext::new(self.db, self.db, body_ref, body)
                .methods()
                .method_candidates_for_ty(ty)?,
        ))
    }
}
