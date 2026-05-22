//! Body IR autoderef candidate generation.
//!
//! This is the narrow adjustment layer between expression types and the contexts that can look
//! through references. Trait-backed `Deref` can extend the receiver modes later without changing
//! call sites that only want explicit reference peeling.

use crate::ir::ty::{BodyRefMutability, BodyTy};

const AUTODEREF_LIMIT: usize = 8;

/// Computes adjusted Body IR types for contexts that may look through references.
pub struct BodyAutoderef;

impl BodyAutoderef {
    /// Returns candidate types in lookup order for the requested adjustment context.
    pub fn candidates<'ty>(
        mode: BodyAutoderefMode,
        ty: &'ty BodyTy,
    ) -> BodyAutoderefCandidates<'ty> {
        match mode {
            BodyAutoderefMode::PeelReferences
            | BodyAutoderefMode::FieldLookup
            | BodyAutoderefMode::MethodReceiver => BodyAutoderefCandidates {
                kind: BodyAutoderefCandidatesKind::References {
                    next_ty: Some(ty),
                    next_depth: 0,
                    next_mutability: None,
                },
            },
            BodyAutoderefMode::ExplicitDeref => BodyAutoderefCandidates {
                kind: BodyAutoderefCandidatesKind::ExplicitDeref { ty: Some(ty) },
            },
        }
    }
}

/// Describes which adjustment rule the caller wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyAutoderefMode {
    /// Peel only explicit `&T` / `&mut T` wrappers.
    ///
    /// This mode is for contexts that want reference transparency without receiver adjustment,
    /// such as inferred type navigation or pattern propagation.
    PeelReferences,
    /// Candidate types used while resolving a field receiver.
    FieldLookup,
    /// Candidate types used while resolving a method receiver.
    MethodReceiver,
    /// Candidate type produced by one explicit `*expr`.
    ExplicitDeref,
}

/// One adjusted candidate type produced by `BodyAutoderef`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyAutoderefCandidate<'ty> {
    ty: &'ty BodyTy,
    depth: usize,
    mutability: Option<BodyRefMutability>,
}

impl<'ty> BodyAutoderefCandidate<'ty> {
    /// The adjusted type visible at this autoderef depth.
    pub fn ty(&self) -> &'ty BodyTy {
        self.ty
    }

    /// Number of reference deref steps applied to reach this candidate.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Mutability of the reference dereferenced to reach this candidate.
    pub fn mutability(&self) -> Option<BodyRefMutability> {
        self.mutability
    }
}

/// Lazy stream of adjusted candidate types.
#[derive(Debug, Clone)]
pub struct BodyAutoderefCandidates<'ty> {
    kind: BodyAutoderefCandidatesKind<'ty>,
}

#[derive(Debug, Clone)]
enum BodyAutoderefCandidatesKind<'ty> {
    References {
        next_ty: Option<&'ty BodyTy>,
        next_depth: usize,
        next_mutability: Option<BodyRefMutability>,
    },
    ExplicitDeref {
        ty: Option<&'ty BodyTy>,
    },
}

impl<'ty> Iterator for BodyAutoderefCandidates<'ty> {
    type Item = BodyAutoderefCandidate<'ty>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.kind {
            BodyAutoderefCandidatesKind::References {
                next_ty,
                next_depth,
                next_mutability,
            } => {
                let ty = next_ty.take()?;
                let candidate = BodyAutoderefCandidate {
                    ty,
                    depth: *next_depth,
                    mutability: *next_mutability,
                };

                if *next_depth >= AUTODEREF_LIMIT {
                    return Some(candidate);
                }

                if let Some((inner, mutability)) = ty.reference_inner() {
                    *next_ty = Some(inner);
                    *next_depth += 1;
                    *next_mutability = Some(mutability);
                }

                Some(candidate)
            }
            BodyAutoderefCandidatesKind::ExplicitDeref { ty } => {
                let ty = ty.take()?;
                ty.reference_inner()
                    .map(|(inner, mutability)| BodyAutoderefCandidate {
                        ty: inner,
                        depth: 1,
                        mutability: Some(mutability),
                    })
            }
        }
    }
}
