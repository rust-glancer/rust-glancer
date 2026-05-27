//! Body IR autoderef candidate generation.
//!
//! This is the adjustment layer between expression types and the contexts that can look through
//! references or trait-backed `Deref`. Contexts that only want `&T` transparency use
//! `peel_references`, keeping trait deref out of pattern and type-definition queries.

use std::{borrow::Cow, collections::VecDeque};

use rg_def_map::DefMapReadTxn;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::SemanticIrReadTxn;
use rg_ty::IndexedTy;

use super::{deref::BodyDerefResolver, index::SemanticResolutionIndex};

const AUTODEREF_LIMIT: usize = 8;

/// Computes adjusted indexed types for contexts that may dereference a receiver.
#[derive(Clone, Copy)]
pub struct BodyAutoderef<'query, 'db> {
    def_map: &'query DefMapReadTxn<'db>,
    semantic_ir: &'query SemanticIrReadTxn<'db>,
    semantic_index: Option<&'query SemanticResolutionIndex>,
}

impl<'query, 'db> BodyAutoderef<'query, 'db> {
    /// Creates an autoderef engine without a precomputed Semantic IR lookup index.
    pub fn new(
        def_map: &'query DefMapReadTxn<'db>,
        semantic_ir: &'query SemanticIrReadTxn<'db>,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            semantic_index: None,
        }
    }

    /// Creates an autoderef engine that can reuse the body-resolution method lookup index.
    pub(crate) fn with_index(
        def_map: &'query DefMapReadTxn<'db>,
        semantic_ir: &'query SemanticIrReadTxn<'db>,
        semantic_index: &'query SemanticResolutionIndex,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            semantic_index: Some(semantic_index),
        }
    }

    /// Returns candidate types in lookup order for the requested adjustment context.
    pub fn candidates<'ty>(
        self,
        mode: BodyAutoderefMode,
        ty: &'ty IndexedTy,
    ) -> BodyAutoderefCandidates<'query, 'db, 'ty> {
        let kind = match mode {
            BodyAutoderefMode::PeelReferences
            | BodyAutoderefMode::FieldLookup
            | BodyAutoderefMode::MethodReceiver => {
                let mut pending = VecDeque::new();
                pending.push_back(PendingAutoderefCandidate {
                    ty: PendingAutoderefTy::Borrowed(ty),
                    depth: 0,
                    mutability: None,
                });
                BodyAutoderefCandidatesKind::Recursive { mode, pending }
            }
            BodyAutoderefMode::ExplicitDeref => BodyAutoderefCandidatesKind::ExplicitDeref {
                source_ty: Some(ty),
                targets: VecDeque::new(),
            },
        };

        BodyAutoderefCandidates {
            autoderef: self,
            kind,
        }
    }

    /// Peels only explicit `&T` / `&mut T` wrappers.
    pub fn peel_references<'ty>(
        ty: &'ty IndexedTy,
    ) -> impl Iterator<Item = BodyAutoderefCandidate<'ty>> {
        BodyReferencePeelingCandidates {
            next_ty: Some(ty),
            next_depth: 0,
            next_mutability: None,
        }
    }

    fn deref_targets(&self, ty: &IndexedTy) -> Result<Vec<IndexedTy>, PackageStoreError> {
        BodyDerefResolver::new(self.def_map, self.semantic_ir, self.semantic_index)
            .targets_for_ty(ty)
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

impl BodyAutoderefMode {
    fn allows_trait_deref(self) -> bool {
        matches!(self, Self::FieldLookup | Self::MethodReceiver)
    }
}

/// One adjusted candidate type produced by `BodyAutoderef`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyAutoderefCandidate<'ty> {
    ty: Cow<'ty, IndexedTy>,
    depth: usize,
    mutability: Option<rg_ty::RefMutability>,
}

impl<'ty> BodyAutoderefCandidate<'ty> {
    /// The adjusted type visible at this autoderef depth.
    pub fn ty(&self) -> &IndexedTy {
        self.ty.as_ref()
    }

    /// Number of deref steps applied to reach this candidate.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Mutability of the reference dereferenced to reach this candidate.
    pub fn mutability(&self) -> Option<rg_ty::RefMutability> {
        self.mutability
    }
}

/// Lazy stream of adjusted candidate types.
#[derive(Clone)]
pub struct BodyAutoderefCandidates<'query, 'db, 'ty> {
    autoderef: BodyAutoderef<'query, 'db>,
    kind: BodyAutoderefCandidatesKind<'ty>,
}

#[derive(Debug, Clone)]
enum BodyAutoderefCandidatesKind<'ty> {
    Recursive {
        mode: BodyAutoderefMode,
        pending: VecDeque<PendingAutoderefCandidate<'ty>>,
    },
    ExplicitDeref {
        source_ty: Option<&'ty IndexedTy>,
        targets: VecDeque<IndexedTy>,
    },
}

#[derive(Debug, Clone)]
struct PendingAutoderefCandidate<'ty> {
    ty: PendingAutoderefTy<'ty>,
    depth: usize,
    mutability: Option<rg_ty::RefMutability>,
}

#[derive(Debug, Clone)]
enum PendingAutoderefTy<'ty> {
    Borrowed(&'ty IndexedTy),
    Owned(IndexedTy),
}

impl<'ty> PendingAutoderefTy<'ty> {
    fn as_ref(&self) -> &IndexedTy {
        match self {
            Self::Borrowed(ty) => ty,
            Self::Owned(ty) => ty,
        }
    }

    fn into_cow(self) -> Cow<'ty, IndexedTy> {
        match self {
            Self::Borrowed(ty) => Cow::Borrowed(ty),
            Self::Owned(ty) => Cow::Owned(ty),
        }
    }

    fn reference_inner(&self) -> Option<(Self, rg_ty::RefMutability)> {
        match self {
            Self::Borrowed(ty) => ty
                .reference_inner()
                .map(|(inner, mutability)| (Self::Borrowed(inner), mutability)),
            Self::Owned(IndexedTy::Reference { mutability, inner }) => {
                Some((Self::Owned((**inner).clone()), *mutability))
            }
            Self::Owned(_) => None,
        }
    }
}

impl<'query, 'db, 'ty> Iterator for BodyAutoderefCandidates<'query, 'db, 'ty> {
    type Item = Result<BodyAutoderefCandidate<'ty>, PackageStoreError>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.kind {
            BodyAutoderefCandidatesKind::Recursive { mode, pending } => {
                let candidate = pending.pop_front()?;

                if candidate.depth < AUTODEREF_LIMIT {
                    if let Some((inner, mutability)) = candidate.ty.reference_inner() {
                        pending.push_back(PendingAutoderefCandidate {
                            ty: inner,
                            depth: candidate.depth + 1,
                            mutability: Some(mutability),
                        });
                    } else if mode.allows_trait_deref() {
                        match self.autoderef.deref_targets(candidate.ty.as_ref()) {
                            Ok(targets) => {
                                for target in targets {
                                    pending.push_back(PendingAutoderefCandidate {
                                        ty: PendingAutoderefTy::Owned(target),
                                        depth: candidate.depth + 1,
                                        mutability: None,
                                    });
                                }
                            }
                            Err(error) => return Some(Err(error)),
                        }
                    }
                }

                Some(Ok(BodyAutoderefCandidate {
                    ty: candidate.ty.into_cow(),
                    depth: candidate.depth,
                    mutability: candidate.mutability,
                }))
            }
            BodyAutoderefCandidatesKind::ExplicitDeref { source_ty, targets } => {
                if let Some(target) = targets.pop_front() {
                    return Some(Ok(BodyAutoderefCandidate {
                        ty: Cow::Owned(target),
                        depth: 1,
                        mutability: None,
                    }));
                }

                let ty = source_ty.take()?;
                let source = PendingAutoderefTy::Borrowed(ty);
                if let Some((inner, mutability)) = source.reference_inner() {
                    return Some(Ok(BodyAutoderefCandidate {
                        ty: inner.into_cow(),
                        depth: 1,
                        mutability: Some(mutability),
                    }));
                }

                match self.autoderef.deref_targets(ty) {
                    Ok(resolved_targets) => {
                        targets.extend(resolved_targets);
                        targets.pop_front().map(|target| {
                            Ok(BodyAutoderefCandidate {
                                ty: Cow::Owned(target),
                                depth: 1,
                                mutability: None,
                            })
                        })
                    }
                    Err(error) => Some(Err(error)),
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct BodyReferencePeelingCandidates<'ty> {
    next_ty: Option<&'ty IndexedTy>,
    next_depth: usize,
    next_mutability: Option<rg_ty::RefMutability>,
}

impl<'ty> Iterator for BodyReferencePeelingCandidates<'ty> {
    type Item = BodyAutoderefCandidate<'ty>;

    fn next(&mut self) -> Option<Self::Item> {
        let ty = self.next_ty.take()?;
        let candidate = BodyAutoderefCandidate {
            ty: Cow::Borrowed(ty),
            depth: self.next_depth,
            mutability: self.next_mutability,
        };

        if self.next_depth >= AUTODEREF_LIMIT {
            return Some(candidate);
        }

        if let Some((inner, mutability)) = ty.reference_inner() {
            self.next_ty = Some(inner);
            self.next_depth += 1;
            self.next_mutability = Some(mutability);
        }

        Some(candidate)
    }
}
