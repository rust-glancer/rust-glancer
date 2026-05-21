//! Reference-only autoderef candidate generation.
//!
//! This is the narrow adjustment layer between expression types and member lookup. It only knows
//! about `&T` / `&mut T`; trait-backed `Deref` can extend the same candidate stream later.

use crate::ir::ty::{BodyRefMutability, BodyTy};

const AUTODEREF_LIMIT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AutoderefMode {
    FieldLookup,
    MethodReceiver,
    ExplicitDeref,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AutoderefCandidate {
    ty: BodyTy,
    _depth: usize,
    _mutability: Option<BodyRefMutability>,
}

impl AutoderefCandidate {
    pub(super) fn ty(&self) -> &BodyTy {
        &self.ty
    }

    pub(super) fn into_ty(self) -> BodyTy {
        self.ty
    }
}

pub(super) struct Autoderef;

impl Autoderef {
    pub(super) fn candidates(mode: AutoderefMode, ty: &BodyTy) -> Vec<AutoderefCandidate> {
        match mode {
            AutoderefMode::FieldLookup | AutoderefMode::MethodReceiver => {
                Self::receiver_candidates(ty)
            }
            AutoderefMode::ExplicitDeref => {
                Self::explicit_deref_candidate(ty).into_iter().collect()
            }
        }
    }

    fn receiver_candidates(ty: &BodyTy) -> Vec<AutoderefCandidate> {
        let mut candidates = vec![AutoderefCandidate {
            ty: ty.clone(),
            _depth: 0,
            _mutability: None,
        }];
        let mut current = ty;

        for depth in 1..=AUTODEREF_LIMIT {
            let Some((inner, mutability)) = current.reference_inner() else {
                break;
            };
            candidates.push(AutoderefCandidate {
                ty: inner.clone(),
                _depth: depth,
                _mutability: Some(mutability),
            });
            current = inner;
        }

        candidates
    }

    fn explicit_deref_candidate(ty: &BodyTy) -> Option<AutoderefCandidate> {
        ty.reference_inner()
            .map(|(inner, mutability)| AutoderefCandidate {
                ty: inner.clone(),
                _depth: 1,
                _mutability: Some(mutability),
            })
    }
}
