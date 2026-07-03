//! Decode solver answers back into project-native inference types.
//!
//! This is the inverse edge of `lower.rs`: Chalk owns the internal proof term, but callers outside
//! the Chalk adapter should only see rust-glancer inference values. The decoder intentionally
//! accepts only shapes that map directly into today's `InferTy` model. Unsupported Chalk answers
//! return `None` instead of becoming a second, partial inference engine.

use chalk_ir::{GenericArg, GenericArgData, Mutability as ChalkMutability, Scalar, Ty, TyKind};

use super::interner::RgChalkInterner;
use super::projection::{ProjectionAnswerVars, ProjectionVariableEnv};
use crate::inference::{InferGenericArg, InferNominalTy, InferTy};
use crate::{FloatTy, PrimitiveTy, SignedIntTy, UnsignedIntTy};

const INTER: RgChalkInterner = RgChalkInterner;

pub(super) fn infer_ty_from_chalk_projection(
    ty: &Ty<RgChalkInterner>,
    variables: &ProjectionVariableEnv,
    answer_vars: &ProjectionAnswerVars,
) -> Option<InferTy> {
    infer_ty_from_chalk_with_vars(ty, Some((variables, answer_vars)))
}

fn infer_ty_from_chalk_with_vars(
    ty: &Ty<RgChalkInterner>,
    variables: Option<(&ProjectionVariableEnv, &ProjectionAnswerVars)>,
) -> Option<InferTy> {
    match ty.kind(INTER) {
        TyKind::Tuple(0, _) => Some(InferTy::Unit),
        TyKind::Tuple(_, substitution) => {
            let fields = substitution
                .iter(INTER)
                .map(|arg| infer_ty_from_chalk_arg(&arg, variables))
                .collect::<Option<Vec<_>>>()?;
            Some(InferTy::Tuple(fields))
        }
        TyKind::Never => Some(InferTy::Never),
        TyKind::Scalar(scalar) => primitive_from_chalk(*scalar).map(InferTy::Primitive),
        TyKind::Str => Some(InferTy::Primitive(PrimitiveTy::Str)),
        TyKind::Slice(inner) => Some(InferTy::Slice(Box::new(infer_ty_from_chalk_with_vars(
            inner, variables,
        )?))),
        TyKind::Ref(mutability, _, inner) => Some(InferTy::Reference {
            mutability: match mutability {
                ChalkMutability::Mut => rg_ir_model::Mutability::Mutable,
                ChalkMutability::Not => rg_ir_model::Mutability::Shared,
            },
            inner: Box::new(infer_ty_from_chalk_with_vars(inner, variables)?),
        }),
        TyKind::Adt(adt_id, substitution) => {
            let args = substitution
                .iter(INTER)
                .map(|arg| infer_generic_arg_from_chalk(&arg, variables))
                .collect::<Option<Vec<_>>>()?;
            Some(InferTy::Nominal(InferNominalTy {
                def: adt_id.0,
                args,
            }))
        }
        TyKind::BoundVar(bound_var) => {
            let (variables, answer_vars) = variables?;
            if let Some(index) = bound_var.index_if_innermost()
                && let Some(ty) = variables.project_var_ty(index)
            {
                return Some(ty);
            }
            answer_vars
                .as_slice()
                .iter()
                .find_map(|(var, ty)| (*var == *bound_var).then_some(ty.clone()))
        }
        TyKind::Array(_, _)
        | TyKind::Raw(_, _)
        | TyKind::AssociatedType(_, _)
        | TyKind::Alias(_)
        | TyKind::OpaqueType(_, _)
        | TyKind::FnDef(_, _)
        | TyKind::Closure(_, _)
        | TyKind::Coroutine(_, _)
        | TyKind::CoroutineWitness(_, _)
        | TyKind::Foreign(_)
        | TyKind::Function(_)
        | TyKind::InferenceVar(_, _)
        | TyKind::Dyn(_)
        | TyKind::Placeholder(_)
        | TyKind::Error => None,
    }
}

fn infer_ty_from_chalk_arg(
    arg: &GenericArg<RgChalkInterner>,
    variables: Option<(&ProjectionVariableEnv, &ProjectionAnswerVars)>,
) -> Option<InferTy> {
    let GenericArgData::Ty(ty) = arg.data(INTER) else {
        return None;
    };
    infer_ty_from_chalk_with_vars(ty, variables)
}

fn infer_generic_arg_from_chalk(
    arg: &GenericArg<RgChalkInterner>,
    variables: Option<(&ProjectionVariableEnv, &ProjectionAnswerVars)>,
) -> Option<InferGenericArg> {
    match arg.data(INTER) {
        GenericArgData::Ty(ty) => Some(InferGenericArg::Type(Box::new(
            infer_ty_from_chalk_with_vars(ty, variables)?,
        ))),
        GenericArgData::Lifetime(_) => Some(InferGenericArg::Lifetime("_".to_owned())),
        GenericArgData::Const(_) => None,
    }
}

fn primitive_from_chalk(scalar: Scalar) -> Option<PrimitiveTy> {
    match scalar {
        Scalar::Bool => Some(PrimitiveTy::Bool),
        Scalar::Char => Some(PrimitiveTy::Char),
        Scalar::Int(kind) => Some(PrimitiveTy::SignedInt(match kind {
            chalk_ir::IntTy::Isize => SignedIntTy::Isize,
            chalk_ir::IntTy::I8 => SignedIntTy::I8,
            chalk_ir::IntTy::I16 => SignedIntTy::I16,
            chalk_ir::IntTy::I32 => SignedIntTy::I32,
            chalk_ir::IntTy::I64 => SignedIntTy::I64,
            chalk_ir::IntTy::I128 => SignedIntTy::I128,
        })),
        Scalar::Uint(kind) => Some(PrimitiveTy::UnsignedInt(match kind {
            chalk_ir::UintTy::Usize => UnsignedIntTy::Usize,
            chalk_ir::UintTy::U8 => UnsignedIntTy::U8,
            chalk_ir::UintTy::U16 => UnsignedIntTy::U16,
            chalk_ir::UintTy::U32 => UnsignedIntTy::U32,
            chalk_ir::UintTy::U64 => UnsignedIntTy::U64,
            chalk_ir::UintTy::U128 => UnsignedIntTy::U128,
        })),
        Scalar::Float(kind) => match kind {
            chalk_ir::FloatTy::F32 => Some(PrimitiveTy::Float(FloatTy::F32)),
            chalk_ir::FloatTy::F64 => Some(PrimitiveTy::Float(FloatTy::F64)),
            chalk_ir::FloatTy::F16 | chalk_ir::FloatTy::F128 => None,
        },
    }
}
