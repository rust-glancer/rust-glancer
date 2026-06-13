use rg_ir_model::items::TypeRef;
use rg_ir_model::{TraitRef, TypeDefRef};
use rg_std::UniqueVec;
use rg_text::Name;

use super::family::{PlainTyToInferMapper, TyToInferMapper};
use super::table::{InferVarId, InferVarKind};
use crate::{GenericArg, PrimitiveTy, RefMutability, Ty};

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
        mutability: RefMutability,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferNominalTy {
    pub def: TypeDefRef,
    pub args: Vec<InferGenericArg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferOpaqueTraitBound {
    pub trait_ref: TraitRef,
    pub args: Vec<InferGenericArg>,
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
}
