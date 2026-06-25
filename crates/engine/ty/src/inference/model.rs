use rg_ir_model::items::TypeRef;
use rg_ir_model::{TraitRef, TypeDefRef};
use rg_std::UniqueVec;
use rg_text::Name;

use super::family::{PlainTyToInferMapper, TyToInferMapper};
use super::table::{InferVarId, InferVarKind};
use crate::{GenericArg, Mutability, PrimitiveTy, Ty};

/// Inference-aware mirror of `Ty`.
///
/// This type is transient solver state. It can carry variables inside the same shapes persisted
/// `Ty` already supports, then finalize back to `Ty` once inference is done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferTy {
    // Additions to `Ty`: to-be-inferred variables
    Var(InferVarId),
    IntegerVar(InferVarId),
    FloatVar(InferVarId),
    // Matches `Ty`
    Unit,
    Never,
    Primitive(PrimitiveTy),
    Tuple(Vec<InferTy>),
    Array {
        inner: Box<InferTy>,
        len: Option<String>,
    },
    Slice(Box<InferTy>),
    Reference {
        mutability: Mutability,
        inner: Box<InferTy>,
    },
    Opaque {
        bounds: UniqueVec<InferOpaqueTraitBound>,
    },
    Syntax(Box<TypeRef>),
    Nominal(InferNominalTy),
    SelfTy(InferNominalTy),
    Unknown,
}

impl InferTy {
    pub fn from_ty(ty: &Ty) -> Self {
        let mut mapper = PlainTyToInferMapper;
        mapper.map_ty(ty)
    }

    pub(super) fn var_id(&self) -> Option<InferVarId> {
        match self {
            Self::Var(id) | Self::IntegerVar(id) | Self::FloatVar(id) => Some(*id),
            Self::Unit
            | Self::Never
            | Self::Primitive(_)
            | Self::Tuple(_)
            | Self::Array { .. }
            | Self::Slice(_)
            | Self::Reference { .. }
            | Self::Opaque { .. }
            | Self::Syntax(_)
            | Self::Nominal(_)
            | Self::SelfTy(_)
            | Self::Unknown => None,
        }
    }

    pub(super) fn var_for_kind(kind: InferVarKind, id: InferVarId) -> Self {
        match kind {
            InferVarKind::Type => Self::Var(id),
            InferVarKind::Integer => Self::IntegerVar(id),
            InferVarKind::Float => Self::FloatVar(id),
        }
    }

    pub(super) fn contains_var(&self, needle: InferVarId) -> bool {
        match self {
            InferTy::Var(id) | InferTy::IntegerVar(id) | InferTy::FloatVar(id) => *id == needle,
            InferTy::Tuple(fields) => fields.iter().any(|field| field.contains_var(needle)),
            InferTy::Array { inner, .. }
            | InferTy::Slice(inner)
            | InferTy::Reference { inner, .. } => inner.contains_var(needle),
            InferTy::Opaque { bounds } => bounds
                .iter()
                .any(|bound| bound.args.iter().any(|arg| arg.contains_var(needle))),
            InferTy::Nominal(ty) | InferTy::SelfTy(ty) => {
                ty.args.iter().any(|arg| arg.contains_var(needle))
            }
            InferTy::Unit
            | InferTy::Never
            | InferTy::Primitive(_)
            | InferTy::Syntax(_)
            | InferTy::Unknown => false,
        }
    }

    /// Return whether two types can be handled by the same structural branch.
    pub(super) fn same_shape_as(&self, other: &Self) -> bool {
        match (self, other) {
            (InferTy::Var(_), InferTy::Var(_))
            | (InferTy::IntegerVar(_), InferTy::IntegerVar(_))
            | (InferTy::FloatVar(_), InferTy::FloatVar(_))
            | (InferTy::Unit, InferTy::Unit)
            | (InferTy::Never, InferTy::Never)
            | (InferTy::Opaque { .. }, InferTy::Opaque { .. })
            | (InferTy::Slice(_), InferTy::Slice(_))
            | (InferTy::Unknown, InferTy::Unknown) => true,
            (InferTy::Primitive(lhs), InferTy::Primitive(rhs)) => lhs == rhs,
            (InferTy::Tuple(lhs), InferTy::Tuple(rhs)) => lhs.len() == rhs.len(),
            (InferTy::Array { len: lhs_len, .. }, InferTy::Array { len: rhs_len, .. }) => {
                lhs_len == rhs_len
            }
            (
                InferTy::Reference {
                    mutability: lhs_mutability,
                    ..
                },
                InferTy::Reference {
                    mutability: rhs_mutability,
                    ..
                },
            ) => lhs_mutability == rhs_mutability,
            (InferTy::Syntax(lhs), InferTy::Syntax(rhs)) => lhs == rhs,
            (InferTy::Nominal(lhs), InferTy::Nominal(rhs))
            | (InferTy::SelfTy(lhs), InferTy::SelfTy(rhs)) => lhs.same_shape_as(rhs),
            _ => false,
        }
    }

    /// Return whether this type still carries inference variables.
    pub fn has_var(&self) -> bool {
        match self {
            InferTy::Var(_) | InferTy::IntegerVar(_) | InferTy::FloatVar(_) => true,
            InferTy::Tuple(fields) => fields.iter().any(Self::has_var),
            InferTy::Array { inner, .. }
            | InferTy::Slice(inner)
            | InferTy::Reference { inner, .. } => inner.has_var(),
            InferTy::Opaque { bounds } => bounds
                .iter()
                .any(|bound| bound.args.iter().any(InferGenericArg::has_var)),
            InferTy::Nominal(ty) | InferTy::SelfTy(ty) => {
                ty.args.iter().any(InferGenericArg::has_var)
            }
            InferTy::Unit
            | InferTy::Never
            | InferTy::Primitive(_)
            | InferTy::Syntax(_)
            | InferTy::Unknown => false,
        }
    }

    /// Return whether this type still contains unknown type gaps.
    pub fn has_unknown(&self) -> bool {
        match self {
            InferTy::Unknown => true,
            InferTy::Tuple(fields) => fields.iter().any(Self::has_unknown),
            InferTy::Array { inner, .. }
            | InferTy::Slice(inner)
            | InferTy::Reference { inner, .. } => inner.has_unknown(),
            InferTy::Opaque { bounds } => bounds
                .iter()
                .any(|bound| bound.args.iter().any(InferGenericArg::has_unknown)),
            InferTy::Nominal(ty) | InferTy::SelfTy(ty) => {
                ty.args.iter().any(InferGenericArg::has_unknown)
            }
            InferTy::Var(_)
            | InferTy::IntegerVar(_)
            | InferTy::FloatVar(_)
            | InferTy::Unit
            | InferTy::Never
            | InferTy::Primitive(_)
            | InferTy::Syntax(_) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferNominalTy {
    pub def: TypeDefRef,
    pub args: Vec<InferGenericArg>,
}

impl InferNominalTy {
    /// Return whether nominal args can be compared position-by-position.
    pub(super) fn same_shape_as(&self, other: &Self) -> bool {
        self.def == other.def && self.args.len() == other.args.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferOpaqueTraitBound {
    pub trait_ref: TraitRef,
    pub args: Vec<InferGenericArg>,
}

impl InferOpaqueTraitBound {
    /// Return whether same-trait bound args can be compared position-by-position.
    pub(super) fn same_trait_shape_as(&self, other: &Self) -> bool {
        self.trait_ref == other.trait_ref && self.args.len() == other.args.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferGenericArg {
    Type(Box<InferTy>),
    Lifetime(String),
    Const(String),
    FnTraitArgs {
        params: Vec<InferTy>,
        ret: Box<InferTy>,
    },
    AssocType {
        name: Name,
        ty: Option<Box<InferTy>>,
    },
    Unsupported(String),
}

impl InferGenericArg {
    pub(super) fn from_arg(arg: &GenericArg) -> Self {
        let mut mapper = PlainTyToInferMapper;
        mapper.map_generic_arg(arg)
    }

    pub(super) fn contains_var(&self, needle: InferVarId) -> bool {
        match self {
            InferGenericArg::Type(ty) => ty.contains_var(needle),
            InferGenericArg::FnTraitArgs { params, ret } => {
                params.iter().any(|param| param.contains_var(needle)) || ret.contains_var(needle)
            }
            InferGenericArg::AssocType { ty, .. } => {
                ty.as_deref().is_some_and(|ty| ty.contains_var(needle))
            }
            InferGenericArg::Lifetime(_)
            | InferGenericArg::Const(_)
            | InferGenericArg::Unsupported(_) => false,
        }
    }

    /// Return whether two generic args have the same child layout.
    pub(super) fn same_shape_as(&self, other: &Self) -> bool {
        match (self, other) {
            (InferGenericArg::Type(_), InferGenericArg::Type(_)) => true,
            (InferGenericArg::Lifetime(lhs), InferGenericArg::Lifetime(rhs)) => lhs == rhs,
            (InferGenericArg::Const(lhs), InferGenericArg::Const(rhs)) => lhs == rhs,
            (
                InferGenericArg::FnTraitArgs { params: lhs, .. },
                InferGenericArg::FnTraitArgs { params: rhs, .. },
            ) => lhs.len() == rhs.len(),
            (
                InferGenericArg::AssocType {
                    name: lhs_name,
                    ty: lhs_ty,
                },
                InferGenericArg::AssocType {
                    name: rhs_name,
                    ty: rhs_ty,
                },
            ) => lhs_name == rhs_name && lhs_ty.is_some() == rhs_ty.is_some(),
            (InferGenericArg::Unsupported(lhs), InferGenericArg::Unsupported(rhs)) => lhs == rhs,
            _ => false,
        }
    }

    /// Return whether this generic arg still carries inference variables.
    pub fn has_var(&self) -> bool {
        match self {
            InferGenericArg::Type(ty) => ty.has_var(),
            InferGenericArg::FnTraitArgs { params, ret } => {
                params.iter().any(InferTy::has_var) || ret.has_var()
            }
            InferGenericArg::AssocType { ty, .. } => ty.as_deref().is_some_and(InferTy::has_var),
            InferGenericArg::Lifetime(_)
            | InferGenericArg::Const(_)
            | InferGenericArg::Unsupported(_) => false,
        }
    }

    /// Return whether this generic arg still contains unknown type gaps.
    pub fn has_unknown(&self) -> bool {
        match self {
            InferGenericArg::Type(ty) => ty.has_unknown(),
            InferGenericArg::FnTraitArgs { params, ret } => {
                params.iter().any(InferTy::has_unknown) || ret.has_unknown()
            }
            InferGenericArg::AssocType { ty, .. } => {
                ty.as_deref().is_some_and(InferTy::has_unknown)
            }
            InferGenericArg::Lifetime(_)
            | InferGenericArg::Const(_)
            | InferGenericArg::Unsupported(_) => false,
        }
    }
}
