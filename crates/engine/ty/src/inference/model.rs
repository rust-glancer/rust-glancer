use rg_ir_model::items::TypeRef;
use rg_ir_model::{TraitRef, TypeDefRef};
use rg_std::UniqueVec;
use rg_text::Name;

use super::table::{InferVarId, InferVarKind};
use crate::{GenericArg, NominalTy, OpaqueTraitBound, PrimitiveTy, RefMutability, Ty};

/// Inference-aware mirror of `Ty`.
///
/// This type is transient solver state. It can carry variables inside the same shapes persisted
/// `Ty` already supports, then finalize back to `Ty` once inference is done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferTy {
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
        mutability: RefMutability,
        inner: Box<InferTy>,
    },
    Opaque {
        bounds: UniqueVec<InferOpaqueTraitBound>,
    },
    Syntax(Box<TypeRef>),
    Nominal(InferNominalTy),
    SelfTy(InferNominalTy),
    Var(InferVarId),
    IntegerVar(InferVarId),
    FloatVar(InferVarId),
    Unknown,
}

impl InferTy {
    pub fn from_ty(ty: &Ty) -> Self {
        match ty {
            Ty::Unit => Self::Unit,
            Ty::Never => Self::Never,
            Ty::Primitive(primitive) => Self::Primitive(*primitive),
            Ty::Tuple(fields) => Self::Tuple(fields.iter().map(Self::from_ty).collect()),
            Ty::Array { inner, len } => Self::Array {
                inner: Box::new(Self::from_ty(inner)),
                len: len.clone(),
            },
            Ty::Slice(inner) => Self::Slice(Box::new(Self::from_ty(inner))),
            Ty::Reference { mutability, inner } => Self::Reference {
                mutability: *mutability,
                inner: Box::new(Self::from_ty(inner)),
            },
            Ty::Opaque { bounds } => Self::Opaque {
                bounds: bounds
                    .iter()
                    .map(InferOpaqueTraitBound::from_bound)
                    .collect(),
            },
            Ty::Syntax(ty) => Self::Syntax(Box::new(ty.clone())),
            Ty::Nominal(ty) => Self::Nominal(InferNominalTy::from_nominal_ty(ty)),
            Ty::SelfTy(ty) => Self::SelfTy(InferNominalTy::from_nominal_ty(ty)),
            Ty::Unknown => Self::Unknown,
        }
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferNominalTy {
    pub def: TypeDefRef,
    pub args: Vec<InferGenericArg>,
}

impl InferNominalTy {
    fn from_nominal_ty(ty: &NominalTy) -> Self {
        Self {
            def: ty.def,
            args: ty.args.iter().map(InferGenericArg::from_arg).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferOpaqueTraitBound {
    pub trait_ref: TraitRef,
    pub args: Vec<InferGenericArg>,
}

impl InferOpaqueTraitBound {
    fn from_bound(bound: &OpaqueTraitBound) -> Self {
        Self {
            trait_ref: bound.trait_ref,
            args: bound.args.iter().map(InferGenericArg::from_arg).collect(),
        }
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
        match arg {
            GenericArg::Type(ty) => Self::Type(Box::new(InferTy::from_ty(ty))),
            GenericArg::Lifetime(lifetime) => Self::Lifetime(lifetime.clone()),
            GenericArg::Const(value) => Self::Const(value.clone()),
            GenericArg::FnTraitArgs { params, ret } => Self::FnTraitArgs {
                params: params.iter().map(InferTy::from_ty).collect(),
                ret: Box::new(InferTy::from_ty(ret)),
            },
            GenericArg::AssocType { name, ty } => Self::AssocType {
                name: name.clone(),
                ty: ty
                    .as_ref()
                    .map(|ty| Box::new(InferTy::from_ty(ty.as_ref()))),
            },
            GenericArg::Unsupported(text) => Self::Unsupported(text.clone()),
        }
    }
}
