//! Body-aware resolution adapter for view projections.
//!
//! Body resolution still lives in `rg_body_ir`, but view modules should not each know how to
//! construct its context. This adapter is the single `ir-view` entry point for body-local path and
//! member facts.

use rg_body_ir::{BodyResolutionContext, ResolvedBodyData};
use rg_ir_model::{BodyRef, Path, ScopeId, TypePathResolution, identity::DeclarationRef};
use rg_ir_storage::ItemLookupIndex;
use rg_ty::{MemberMethodCandidateRef, Ty};

use crate::IndexedViewDb;

/// Runs body-aware resolution queries for view projections.
pub(crate) struct BodyResolutionView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> BodyResolutionView<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    fn body_with_index(
        &self,
        body_ref: BodyRef,
    ) -> anyhow::Result<Option<(&ResolvedBodyData, &ItemLookupIndex)>> {
        let Some(target_bodies) = self.db.body_ir.target_bodies(body_ref.target)? else {
            return Ok(None);
        };
        let Some(body) = target_bodies.body(body_ref.body) else {
            return Ok(None);
        };

        Ok(Some((body, target_bodies.semantic_index())))
    }

    /// Resolve a type path in a body scope.
    pub(crate) fn type_path_resolution(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Option<TypePathResolution>> {
        let Some((body, semantic_index)) = self.body_with_index(body_ref)? else {
            return Ok(None);
        };

        Ok(Some(
            BodyResolutionContext::new(self.db, self.db, body_ref, body, semantic_index)
                .type_path_query()
                .resolve_in_scope(scope, path)?,
        ))
    }

    /// Find declarations for a body value path without local binding ordering.
    pub(crate) fn nonlocal_value_path_declarations(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let Some((body, semantic_index)) = self.body_with_index(body_ref)? else {
            return Ok(Vec::new());
        };

        Ok(
            BodyResolutionContext::new(self.db, self.db, body_ref, body, semantic_index)
                .value_paths()
                .resolve_nonlocal_path_declarations(scope, path)?,
        )
    }

    /// Resolve the type of a body value path without local binding ordering.
    pub(crate) fn nonlocal_value_path_ty(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Ty> {
        let Some((body, semantic_index)) = self.body_with_index(body_ref)? else {
            return Ok(Ty::Unknown);
        };

        Ok(
            BodyResolutionContext::new(self.db, self.db, body_ref, body, semantic_index)
                .value_paths()
                .resolve_nonlocal_path_ty(scope, path)?,
        )
    }

    /// Return body-aware method refs for a receiver type.
    pub(crate) fn method_candidate_refs_for_ty(
        &self,
        body_ref: BodyRef,
        ty: &Ty,
    ) -> anyhow::Result<Option<Vec<MemberMethodCandidateRef>>> {
        let Some((body, semantic_index)) = self.body_with_index(body_ref)? else {
            return Ok(None);
        };

        Ok(Some(
            BodyResolutionContext::new(self.db, self.db, body_ref, body, semantic_index)
                .methods()
                .method_candidates_for_ty(ty)?,
        ))
    }
}
