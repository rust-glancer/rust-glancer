//! Receiver-method lookup across crate-level and body-local impl universes.

use anyhow::Context as _;
use rg_ir_model::{BodyRef, CrateRef};
use rg_ty::{MemberMethodCandidateRef, MemberQuery, Ty, TyContext};

use super::{MemberFunction, MemberMethodCandidate, MemberUseSite, MemberView};
use crate::{body::BodyResolutionView, ty::IndexedType};

impl<'a, 'db> MemberView<'a, 'db> {
    /// Return methods visible for a type at a crate or body use site.
    pub fn method_candidates_for_ty<'view>(
        &'view self,
        use_site: MemberUseSite,
        ty: &IndexedType,
    ) -> anyhow::Result<Vec<MemberMethodCandidate<'view>>> {
        let candidates = self
            .method_candidate_refs_for_ty(use_site, ty.raw())
            .context("resolve method candidate references")?;
        self.method_candidates_from_refs(candidates)
            .context("project method candidates")
    }

    /// Return method refs before loading borrowed function data.
    fn method_candidate_refs_for_ty(
        &self,
        use_site: MemberUseSite,
        ty: &Ty,
    ) -> anyhow::Result<Vec<MemberMethodCandidateRef>> {
        match use_site {
            MemberUseSite::Crate(crate_ref) => {
                self.crate_method_candidate_refs_for_ty(crate_ref, ty)
            }
            MemberUseSite::Body(body) => self.body_method_candidate_refs_for_ty(body, ty),
        }
    }

    /// Return crate-level method refs.
    fn crate_method_candidate_refs_for_ty(
        &self,
        use_site: CrateRef,
        ty: &Ty,
    ) -> anyhow::Result<Vec<MemberMethodCandidateRef>> {
        let Some(item_lookup_index) = self
            .db
            .body_ir
            .item_lookup_index(use_site)
            .context("read method candidate item index")?
        else {
            return Ok(Vec::new());
        };
        let member_query = MemberQuery::new(TyContext::new(
            self.db,
            self.db,
            item_lookup_index,
            self.db.trait_selection(use_site),
        ));
        member_query
            .method_candidates_for_ty(ty)
            .context("resolve crate method candidates")
    }

    /// Return body-aware method refs, falling back to crate-level refs if the body is absent.
    fn body_method_candidate_refs_for_ty(
        &self,
        body: BodyRef,
        ty: &Ty,
    ) -> anyhow::Result<Vec<MemberMethodCandidateRef>> {
        let Some(candidates) = BodyResolutionView::new(self.db)
            .method_candidate_refs_for_ty(body, ty)
            .context("resolve body method candidates")?
        else {
            // Missing body facts should not hide crate-level methods from editor queries.
            return self.crate_method_candidate_refs_for_ty(body.crate_ref, ty);
        };

        Ok(candidates)
    }

    /// Load function data for method refs and keep candidates whose functions still exist.
    fn method_candidates_from_refs<'view>(
        &'view self,
        candidates: Vec<MemberMethodCandidateRef>,
    ) -> anyhow::Result<Vec<MemberMethodCandidate<'view>>> {
        let mut methods = Vec::new();
        for candidate in candidates {
            let Some(function) = self
                .function(candidate.function())
                .context("read method candidate function")?
            else {
                continue;
            };
            methods.push(Self::method_candidate(function, candidate));
        }

        Ok(methods)
    }

    /// Combine borrowed function data with lookup origin.
    fn method_candidate<'view>(
        function: MemberFunction<'view>,
        candidate: MemberMethodCandidateRef,
    ) -> MemberMethodCandidate<'view> {
        MemberMethodCandidate {
            function,
            origin: candidate.origin().into(),
        }
    }
}
