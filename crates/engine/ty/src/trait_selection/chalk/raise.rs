//! Decode solver answers back into project-native inference types.
//!
//! This is the inverse edge of `lower.rs`: Chalk owns the internal proof term, but callers outside
//! the Chalk adapter should only see rust-glancer inference values. The decoder intentionally
//! accepts only shapes that map directly into today's `Ty` model. Unsupported Chalk answers
//! return `None` instead of becoming a second, partial inference engine.

use chalk_ir::{
    Const, ConstValue, GenericArg as ChalkGenericArg, GenericArgData,
    Mutability as ChalkMutability, Scalar, Ty as ChalkTy, TyKind,
};

use super::interner::RgChalkInterner;
use super::projection::{ProjectionAnswerVars, ProjectionVariableEnv};
use crate::{FloatTy, GenericArg, NominalTy, PrimitiveTy, SignedIntTy, Ty, UnsignedIntTy};

const INTER: RgChalkInterner = RgChalkInterner;

pub(super) fn infer_ty_from_chalk_projection(
    ty: &ChalkTy<RgChalkInterner>,
    variables: &ProjectionVariableEnv,
    answer_vars: &ProjectionAnswerVars,
) -> Option<Ty> {
    infer_ty_from_chalk_with_vars(ty, Some((variables, answer_vars)))
}

fn infer_ty_from_chalk_with_vars(
    ty: &ChalkTy<RgChalkInterner>,
    variables: Option<(&ProjectionVariableEnv, &ProjectionAnswerVars)>,
) -> Option<Ty> {
    match ty.kind(INTER) {
        TyKind::Tuple(0, _) => Some(Ty::Unit),
        TyKind::Tuple(_, substitution) => {
            let fields = substitution
                .iter(INTER)
                .map(|arg| infer_ty_from_chalk_arg(arg, variables))
                .collect::<Option<Vec<_>>>()?;
            Some(Ty::Tuple(fields))
        }
        TyKind::Never => Some(Ty::Never),
        TyKind::Scalar(scalar) => primitive_from_chalk(*scalar).map(Ty::Primitive),
        TyKind::Str => Some(Ty::Primitive(PrimitiveTy::Str)),
        TyKind::Slice(inner) => Some(Ty::Slice(Box::new(infer_ty_from_chalk_with_vars(
            inner, variables,
        )?))),
        TyKind::Array(inner, len) => Some(Ty::Array {
            inner: Box::new(infer_ty_from_chalk_with_vars(inner, variables)?),
            len: Some(array_len_from_chalk(len)?),
        }),
        TyKind::Ref(mutability, _, inner) => Some(Ty::Reference {
            mutability: match mutability {
                ChalkMutability::Mut => rg_ir_model::Mutability::Mutable,
                ChalkMutability::Not => rg_ir_model::Mutability::Shared,
            },
            inner: Box::new(infer_ty_from_chalk_with_vars(inner, variables)?),
        }),
        TyKind::Adt(adt_id, substitution) => {
            let args = substitution
                .iter(INTER)
                .map(|arg| infer_generic_arg_from_chalk(arg, variables))
                .collect::<Option<Vec<_>>>()?;
            Some(Ty::Nominal(NominalTy {
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
        TyKind::Raw(_, _)
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

fn array_len_from_chalk(len: &Const<RgChalkInterner>) -> Option<String> {
    let ConstValue::Concrete(value) = &len.data(INTER).value else {
        return None;
    };
    Some(value.interned.clone())
}

fn infer_ty_from_chalk_arg(
    arg: &ChalkGenericArg<RgChalkInterner>,
    variables: Option<(&ProjectionVariableEnv, &ProjectionAnswerVars)>,
) -> Option<Ty> {
    let GenericArgData::Ty(ty) = arg.data(INTER) else {
        return None;
    };
    infer_ty_from_chalk_with_vars(ty, variables)
}

fn infer_generic_arg_from_chalk(
    arg: &ChalkGenericArg<RgChalkInterner>,
    variables: Option<(&ProjectionVariableEnv, &ProjectionAnswerVars)>,
) -> Option<GenericArg> {
    match arg.data(INTER) {
        GenericArgData::Ty(ty) => Some(GenericArg::Type(Box::new(infer_ty_from_chalk_with_vars(
            ty, variables,
        )?))),
        GenericArgData::Lifetime(_) => Some(GenericArg::Lifetime(rg_text::Name::new("_"))),
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
