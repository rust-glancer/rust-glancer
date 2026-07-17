//! Body-aware resolution adapter for view projections.
//!
//! Body resolution still lives in `rg_body_ir`, but view modules should not each know how to
//! construct its context. This adapter is the single `ir-view` entry point for body-local path and
//! member facts.

use rg_body_ir::{BodyResolutionContext, BodyView};
use rg_ir_model::{
    BodyRef, EnumVariantRef, Path, ScopeId, TypePathResolution, identity::DeclarationRef,
};
use rg_semantic_ir::ItemLookupIndex;
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
    ) -> anyhow::Result<Option<(BodyView<'_>, &ItemLookupIndex)>> {
        let Some(body) = self.db.body_ir.body(body_ref)? else {
            return Ok(None);
        };
        let Some(semantic_index) = self.db.body_ir.semantic_index(body_ref.crate_ref)? else {
            return Ok(None);
        };

        Ok(Some((body, semantic_index)))
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
        let trait_selection = self.db.trait_selection(body_ref.crate_ref);

        Ok(Some(
            BodyResolutionContext::new(
                self.db,
                self.db,
                body_ref,
                body,
                semantic_index,
                trait_selection,
            )
            .type_path_query()
            .resolve_in_scope(scope, path)?,
        ))
    }

    /// Resolve an enum variant selected through a body-local type path.
    pub(crate) fn type_path_enum_variant(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Option<EnumVariantRef>> {
        let Some((body, semantic_index)) = self.body_with_index(body_ref)? else {
            return Ok(None);
        };
        let trait_selection = self.db.trait_selection(body_ref.crate_ref);

        BodyResolutionContext::new(
            self.db,
            self.db,
            body_ref,
            body,
            semantic_index,
            trait_selection,
        )
        .type_path_query()
        .resolve_enum_variant_in_scope(scope, path)
        .map_err(Into::into)
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
        let trait_selection = self.db.trait_selection(body_ref.crate_ref);

        Ok(BodyResolutionContext::new(
            self.db,
            self.db,
            body_ref,
            body,
            semantic_index,
            trait_selection,
        )
        .value_paths()
        .resolve_nonlocal_path_declarations(scope, path)?)
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
        let trait_selection = self.db.trait_selection(body_ref.crate_ref);

        Ok(BodyResolutionContext::new(
            self.db,
            self.db,
            body_ref,
            body,
            semantic_index,
            trait_selection,
        )
        .value_paths()
        .resolve_nonlocal_path_ty(scope, path)?)
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
        let trait_selection = self.db.trait_selection(body_ref.crate_ref);

        Ok(Some(
            BodyResolutionContext::new(
                self.db,
                self.db,
                body_ref,
                body,
                semantic_index,
                trait_selection,
            )
            .methods()
            .method_candidates_for_ty(ty)?,
        ))
    }
}
