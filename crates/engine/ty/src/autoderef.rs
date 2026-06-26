//! Autoderef candidate generation over item/path query providers.
//!
//! This is the adjustment layer between expression types and the contexts that can look through
//! references or trait-backed `Deref`. Contexts that only want `&T` transparency use
//! `ReferencePeelingCandidates`, keeping trait deref out of pattern and type-definition queries.

use std::{borrow::Cow, collections::VecDeque};

use rg_ir_storage::{DefMapSource, ItemLookupIndex, ItemStoreSource, TargetItemQuery};

use crate::{ItemPathQuery, Mutability, Ty, deref::DerefResolver};
use rg_std::UniqueVec;

const AUTODEREF_LIMIT: usize = 8;

/// Computes adjusted types for contexts that may dereference a receiver.
#[derive(Clone)]
pub struct Autoderef<'query, D, I> {
    item_paths: ItemPathQuery<'query, D, I>,
    target_items: TargetItemQuery<'query, D, I>,
    lookup_index: &'query ItemLookupIndex,
}

impl<'query, D, I> Autoderef<'query, D, I>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
{
    /// Creates an autoderef engine over a target-scoped receiver lookup index.
    pub fn with_index(
        item_paths: ItemPathQuery<'query, D, I>,
        target_items: TargetItemQuery<'query, D, I>,
        lookup_index: &'query ItemLookupIndex,
    ) -> Self {
        Self {
            item_paths,
            target_items,
            lookup_index,
        }
    }

    /// Returns candidate types in lookup order for the requested adjustment context.
    pub fn candidates<'ty>(
        self,
        mode: AutoderefMode,
        ty: &'ty Ty,
    ) -> AutoderefCandidates<'query, 'ty, D, I> {
        let kind = match mode {
            AutoderefMode::PeelReferences
            | AutoderefMode::FieldLookup
            | AutoderefMode::MethodReceiver => {
                let mut pending = VecDeque::new();
                pending.push_back(PendingAutoderefCandidate {
                    ty: PendingAutoderefTy::Borrowed(ty),
                    depth: 0,
                    mutability: None,
                });
                AutoderefCandidatesKind::Recursive { mode, pending }
            }
            AutoderefMode::ExplicitDeref => AutoderefCandidatesKind::ExplicitDeref {
                source_ty: Some(ty),
                targets: VecDeque::new(),
            },
        };

        AutoderefCandidates {
            autoderef: self,
            kind,
        }
    }

    fn deref_targets(&self, ty: &Ty) -> Result<UniqueVec<Ty>, D::Error> {
        DerefResolver::new(
            self.item_paths.clone(),
            self.target_items.clone(),
            self.lookup_index,
        )
        .targets_for_ty(ty)
    }
}

/// Describes which adjustment rule the caller wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoderefMode {
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

impl AutoderefMode {
    fn allows_trait_deref(self) -> bool {
        matches!(self, Self::FieldLookup | Self::MethodReceiver)
    }

    fn allows_array_to_slice_adjustment(self) -> bool {
        matches!(self, Self::MethodReceiver)
    }
}

/// One adjusted candidate type produced by `Autoderef`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoderefCandidate<'ty> {
    ty: Cow<'ty, Ty>,
    depth: usize,
    mutability: Option<Mutability>,
}

impl<'ty> AutoderefCandidate<'ty> {
    /// The adjusted type visible at this autoderef depth.
    pub fn ty(&self) -> &Ty {
        self.ty.as_ref()
    }

    /// Number of deref steps applied to reach this candidate.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Mutability of the reference dereferenced to reach this candidate.
    pub fn mutability(&self) -> Option<Mutability> {
        self.mutability
    }
}

/// Lazy stream of adjusted candidate types.
#[derive(Clone)]
pub struct AutoderefCandidates<'query, 'ty, D, I> {
    autoderef: Autoderef<'query, D, I>,
    kind: AutoderefCandidatesKind<'ty>,
}

#[derive(Debug, Clone)]
enum AutoderefCandidatesKind<'ty> {
    Recursive {
        mode: AutoderefMode,
        pending: VecDeque<PendingAutoderefCandidate<'ty>>,
    },
    ExplicitDeref {
        source_ty: Option<&'ty Ty>,
        targets: VecDeque<Ty>,
    },
}

#[derive(Debug, Clone)]
struct PendingAutoderefCandidate<'ty> {
    ty: PendingAutoderefTy<'ty>,
    depth: usize,
    mutability: Option<Mutability>,
}

#[derive(Debug, Clone)]
enum PendingAutoderefTy<'ty> {
    Borrowed(&'ty Ty),
    Owned(Ty),
}

impl<'ty> PendingAutoderefTy<'ty> {
    fn as_ref(&self) -> &Ty {
        match self {
            Self::Borrowed(ty) => ty,
            Self::Owned(ty) => ty,
        }
    }

    fn into_cow(self) -> Cow<'ty, Ty> {
        match self {
            Self::Borrowed(ty) => Cow::Borrowed(ty),
            Self::Owned(ty) => Cow::Owned(ty),
        }
    }

    fn reference_inner(&self) -> Option<(Self, Mutability)> {
        match self {
            Self::Borrowed(ty) => ty
                .reference_inner()
                .map(|(inner, mutability)| (Self::Borrowed(inner), mutability)),
            Self::Owned(Ty::Reference { mutability, inner }) => {
                Some((Self::Owned((**inner).clone()), *mutability))
            }
            Self::Owned(_) => None,
        }
    }

    fn array_to_slice_adjustment(&self) -> Option<Self> {
        match self.as_ref() {
            // Array-to-slice is a builtin receiver adjustment, not a trait-backed deref. Returning
            // an owned slice candidate lets method lookup reuse ordinary `impl<T> [T]` handling.
            Ty::Array { inner, .. } => Some(Self::Owned(Ty::slice((**inner).clone()))),
            _ => None,
        }
    }
}

impl<'query, 'ty, D, I> Iterator for AutoderefCandidates<'query, 'ty, D, I>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
{
    type Item = Result<AutoderefCandidate<'ty>, D::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.kind {
            AutoderefCandidatesKind::Recursive { mode, pending } => {
                let candidate = pending.pop_front()?;

                if candidate.depth < AUTODEREF_LIMIT {
                    if let Some((inner, mutability)) = candidate.ty.reference_inner() {
                        pending.push_back(PendingAutoderefCandidate {
                            ty: inner,
                            depth: candidate.depth + 1,
                            mutability: Some(mutability),
                        });
                    } else if mode.allows_array_to_slice_adjustment()
                        && let Some(slice) = candidate.ty.array_to_slice_adjustment()
                    {
                        pending.push_back(PendingAutoderefCandidate {
                            ty: slice,
                            depth: candidate.depth + 1,
                            mutability: candidate.mutability,
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

                Some(Ok(AutoderefCandidate {
                    ty: candidate.ty.into_cow(),
                    depth: candidate.depth,
                    mutability: candidate.mutability,
                }))
            }
            AutoderefCandidatesKind::ExplicitDeref { source_ty, targets } => {
                if let Some(target) = targets.pop_front() {
                    return Some(Ok(AutoderefCandidate {
                        ty: Cow::Owned(target),
                        depth: 1,
                        mutability: None,
                    }));
                }

                let ty = source_ty.take()?;
                let source = PendingAutoderefTy::Borrowed(ty);
                if let Some((inner, mutability)) = source.reference_inner() {
                    return Some(Ok(AutoderefCandidate {
                        ty: inner.into_cow(),
                        depth: 1,
                        mutability: Some(mutability),
                    }));
                }

                match self.autoderef.deref_targets(ty) {
                    Ok(resolved_targets) => {
                        targets.extend(resolved_targets);
                        targets.pop_front().map(|target| {
                            Ok(AutoderefCandidate {
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
pub struct ReferencePeelingCandidates<'ty> {
    next_ty: Option<&'ty Ty>,
    next_depth: usize,
    next_mutability: Option<Mutability>,
}

impl<'ty> ReferencePeelingCandidates<'ty> {
    /// Peels only explicit `&T` / `&mut T` wrappers.
    pub fn new(ty: &'ty Ty) -> Self {
        Self {
            next_ty: Some(ty),
            next_depth: 0,
            next_mutability: None,
        }
    }
}

impl<'ty> Iterator for ReferencePeelingCandidates<'ty> {
    type Item = AutoderefCandidate<'ty>;

    fn next(&mut self) -> Option<Self::Item> {
        let ty = self.next_ty.take()?;
        let candidate = AutoderefCandidate {
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
