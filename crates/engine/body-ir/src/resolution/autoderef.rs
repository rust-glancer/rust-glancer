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
pub struct BodyAutoderefCandidate {
    ty: BodyTy,
    depth: usize,
    mutability: Option<BodyRefMutability>,
}

impl BodyAutoderefCandidate {
    pub fn ty(&self) -> &BodyTy {
        &self.ty
    }

    pub fn into_ty(self) -> BodyTy {
        self.ty
    }

    pub fn depth(&self) -> usize {
        self.depth
    }

    pub fn mutability(&self) -> Option<BodyRefMutability> {
        self.mutability
    }
}

pub struct BodyAutoderef;

impl BodyAutoderef {
    pub(super) fn candidates(mode: AutoderefMode, ty: &BodyTy) -> Vec<BodyAutoderefCandidate> {
        match mode {
            AutoderefMode::FieldLookup | AutoderefMode::MethodReceiver => {
                Self::receiver_candidates(ty)
            }
            AutoderefMode::ExplicitDeref => {
                Self::explicit_deref_candidate(ty).into_iter().collect()
            }
        }
    }

    pub fn receiver_candidates(ty: &BodyTy) -> Vec<BodyAutoderefCandidate> {
        let mut candidates = vec![BodyAutoderefCandidate {
            ty: ty.clone(),
            depth: 0,
            mutability: None,
        }];
        let mut current = ty;

        for depth in 1..=AUTODEREF_LIMIT {
            let Some((inner, mutability)) = current.reference_inner() else {
                break;
            };
            candidates.push(BodyAutoderefCandidate {
                ty: inner.clone(),
                depth,
                mutability: Some(mutability),
            });
            current = inner;
        }

        candidates
    }

    pub fn explicit_deref_candidate(ty: &BodyTy) -> Option<BodyAutoderefCandidate> {
        ty.reference_inner()
            .map(|(inner, mutability)| BodyAutoderefCandidate {
                ty: inner.clone(),
                depth: 1,
                mutability: Some(mutability),
            })
    }
}
