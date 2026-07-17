//! Decode solver answers back into project-native inference types.
//!
//! This is the inverse edge of `lower.rs`: Chalk owns the internal proof term, but callers outside
//! the Chalk adapter should only see rust-glancer inference values. The decoder intentionally
//! accepts only shapes that map directly into today's `Ty` model. Unsupported Chalk answers
//! return `None` instead of becoming a second, partial inference engine.

use chalk_ir::{
    AliasTy as ChalkAliasTy, ConstValue as ChalkConstValue, GenericArg as ChalkGenericArg,
    GenericArgData, LifetimeData, Mutability as ChalkMutability, Safety, Scalar, Ty as ChalkTy,
    TyKind,
};

use super::interner::{ChalkDefId, RgChalkInterner};
use super::projection::{ProjectionAnswerVars, ProjectionVariableEnv};
use crate::{
    AdtTy, AliasTy, ConstValue, FloatTy, FnDefTy, GenericArg, GenericArgs, Lifetime, OpaqueTy,
    PrimitiveTy, ProjectionTy, SignedIntTy, Ty, UnsignedIntTy,
};

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
            len: const_value_from_chalk(&len.data(INTER).value)?,
        }),
        TyKind::Ref(mutability, lifetime, inner) => Some(Ty::Reference {
            lifetime: lifetime_from_chalk(lifetime.data(INTER))?,
            mutability: mutability_from_chalk(*mutability),
            inner: Box::new(infer_ty_from_chalk_with_vars(inner, variables)?),
        }),
        TyKind::Raw(mutability, inner) => Some(Ty::RawPointer {
            mutability: mutability_from_chalk(*mutability),
            inner: Box::new(infer_ty_from_chalk_with_vars(inner, variables)?),
        }),
        TyKind::Adt(adt_id, substitution) => {
            let args = substitution
                .iter(INTER)
                .map(|arg| infer_generic_arg_from_chalk(arg, variables))
                .collect::<Option<Vec<_>>>()?;
            Some(Ty::Adt(AdtTy {
                def: adt_id.0,
                args: args.into(),
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
        TyKind::AssociatedType(associated_ty, substitution) => {
            projection_from_chalk(associated_ty.0, substitution, variables)
        }
        TyKind::Alias(alias) => match alias {
            ChalkAliasTy::Projection(alias) => {
                projection_from_chalk(alias.associated_ty_id.0, &alias.substitution, variables)
            }
            ChalkAliasTy::Opaque(alias) => {
                opaque_from_chalk(alias.opaque_ty_id.0, &alias.substitution, variables)
            }
        },
        TyKind::OpaqueType(opaque, substitution) => {
            opaque_from_chalk(opaque.0, substitution, variables)
        }
        TyKind::FnDef(function, substitution) => {
            let ChalkDefId::Function(def) = function.0 else {
                return None;
            };
            Some(Ty::FnDef(FnDefTy {
                def,
                args: generic_args_from_chalk(substitution, variables)?,
            }))
        }
        TyKind::Function(function) => {
            if function.num_binders != 0
                || function.sig.safety != Safety::Safe
                || function.sig.variadic
            {
                return None;
            }
            let mut signature = function
                .substitution
                .0
                .iter(INTER)
                .map(|arg| infer_ty_from_chalk_arg(arg, variables))
                .collect::<Option<Vec<_>>>()?;
            let ret = signature.pop()?;
            Some(Ty::fn_pointer(signature, ret))
        }
        TyKind::Closure(_, _)
        | TyKind::Coroutine(_, _)
        | TyKind::CoroutineWitness(_, _)
        | TyKind::Foreign(_)
        | TyKind::InferenceVar(_, _)
        | TyKind::Dyn(_)
        | TyKind::Placeholder(_)
        | TyKind::Error => None,
    }
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
        GenericArgData::Lifetime(lifetime) => Some(GenericArg::Lifetime(lifetime_from_chalk(
            lifetime.data(INTER),
        )?)),
        GenericArgData::Const(value) => Some(GenericArg::Const(const_value_from_chalk(
            &value.data(INTER).value,
        )?)),
    }
}

fn generic_args_from_chalk(
    substitution: &chalk_ir::Substitution<RgChalkInterner>,
    variables: Option<(&ProjectionVariableEnv, &ProjectionAnswerVars)>,
) -> Option<GenericArgs> {
    substitution
        .iter(INTER)
        .map(|arg| infer_generic_arg_from_chalk(arg, variables))
        .collect()
}

fn projection_from_chalk(
    associated_ty: ChalkDefId,
    substitution: &chalk_ir::Substitution<RgChalkInterner>,
    variables: Option<(&ProjectionVariableEnv, &ProjectionAnswerVars)>,
) -> Option<Ty> {
    let ChalkDefId::AssocType(associated_ty) = associated_ty else {
        return None;
    };
    Some(Ty::Alias(AliasTy::Projection(ProjectionTy {
        associated_ty,
        args: generic_args_from_chalk(substitution, variables)?,
    })))
}

fn opaque_from_chalk(
    opaque: ChalkDefId,
    substitution: &chalk_ir::Substitution<RgChalkInterner>,
    variables: Option<(&ProjectionVariableEnv, &ProjectionAnswerVars)>,
) -> Option<Ty> {
    let ChalkDefId::Opaque(opaque) = opaque else {
        return None;
    };
    Some(Ty::Alias(AliasTy::Opaque(OpaqueTy {
        opaque,
        args: generic_args_from_chalk(substitution, variables)?,
    })))
}

fn lifetime_from_chalk(lifetime: &LifetimeData<RgChalkInterner>) -> Option<Lifetime> {
    match lifetime {
        LifetimeData::Static => Some(Lifetime::Static),
        LifetimeData::Erased => Some(Lifetime::Erased),
        LifetimeData::BoundVar(_)
        | LifetimeData::InferenceVar(_)
        | LifetimeData::Placeholder(_)
        | LifetimeData::Phantom(_, _)
        | LifetimeData::Error => None,
    }
}

fn const_value_from_chalk(value: &ChalkConstValue<RgChalkInterner>) -> Option<ConstValue> {
    let ChalkConstValue::Concrete(value) = value else {
        return None;
    };
    value.interned.parse().ok().map(ConstValue::Scalar)
}

fn mutability_from_chalk(mutability: ChalkMutability) -> rg_ir_model::Mutability {
    match mutability {
        ChalkMutability::Mut => rg_ir_model::Mutability::Mutable,
        ChalkMutability::Not => rg_ir_model::Mutability::Shared,
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
