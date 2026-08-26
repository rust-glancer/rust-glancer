//! Receiver-method lookup across crate-level and body-local impl universes.

use anyhow::Context as _;
use rg_ir_model::identity::LexicalScopeRef;
use rg_ty::{MemberMethodCandidateRef, Ty};

use super::{MemberFunction, MemberMethodCandidate, MemberView};
use crate::{body::BodyResolutionView, ty::IndexedType};

impl<'a, 'db> MemberView<'a, 'db> {
    /// Return methods visible for a type at one lexical body scope.
    pub fn method_candidates_for_ty<'view>(
        &'view self,
        scope: LexicalScopeRef,
        ty: &IndexedType,
    ) -> anyhow::Result<Vec<MemberMethodCandidate<'view>>> {
        let candidates = self
            .body_method_candidate_refs_for_ty(scope, ty.raw())
            .context("resolve method candidate references")?;
        self.method_candidates_from_refs(candidates)
            .context("project method candidates")
    }

    /// Return body-aware method refs.
    fn body_method_candidate_refs_for_ty(
        &self,
        scope: LexicalScopeRef,
        ty: &Ty,
    ) -> anyhow::Result<Vec<MemberMethodCandidateRef>> {
        let body = scope.body_ir();
        let Some(candidates) = BodyResolutionView::new(self.db)
            .method_candidate_refs_for_ty(body, scope.scope_id(), ty)
            .context("resolve body method candidates")?
        else {
            // Lexical trait scope belongs to the body's DefMap. If those facts disappeared between
            // source-site discovery and lookup, a crate-wide fallback would reintroduce traits
            // that Rust does not make callable here. Fail soft with no methods instead.
            return Ok(Vec::new());
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
