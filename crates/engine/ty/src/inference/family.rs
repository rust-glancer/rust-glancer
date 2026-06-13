//! Shared traversal over the type-like families used by inference.
//!
//! The policy methods are intentionally small: callers provide the interesting replacement logic,
//! while this module owns the repetitive shape walk through types and generic args.

use rg_ir_model::items::{GenericArg as ItemGenericArg, TypePath, TypeRef};
use rg_std::UniqueVec;

use super::model::{InferGenericArg, InferNominalTy, InferOpaqueTraitBound, InferTy};
use crate::{GenericArg, NominalTy, OpaqueTraitBound, Ty};

/// Maps the resolved `Ty` family into the inference `InferTy` family.
pub(crate) trait TyToInferMapper {
    /// Convert one resolved type, recursing through type-bearing children.
    fn map_ty(&mut self, ty: &Ty) -> InferTy {
        match ty {
            Ty::Unit => InferTy::Unit,
            Ty::Never => InferTy::Never,
            Ty::Primitive(primitive) => InferTy::Primitive(*primitive),
            Ty::Tuple(fields) => {
                InferTy::Tuple(fields.iter().map(|field| self.map_ty(field)).collect())
            }
            Ty::Array { inner, len } => InferTy::Array {
                inner: Box::new(self.map_ty(inner)),
                len: len.clone(),
            },
            Ty::Slice(inner) => InferTy::Slice(Box::new(self.map_ty(inner))),
            Ty::Reference { mutability, inner } => InferTy::Reference {
                mutability: *mutability,
                inner: Box::new(self.map_ty(inner)),
            },
            Ty::Opaque { bounds } => InferTy::Opaque {
                bounds: bounds
                    .iter()
                    .map(|bound| self.map_opaque_bound(bound))
                    .collect::<UniqueVec<_>>(),
            },
            Ty::Syntax(ty) => InferTy::Syntax(Box::new(ty.clone())),
            Ty::Nominal(ty) => InferTy::Nominal(self.map_nominal_ty(ty)),
            Ty::SelfTy(ty) => InferTy::SelfTy(self.map_nominal_ty(ty)),
            Ty::Unknown => self.map_unknown_ty(),
        }
    }

    /// Convert nominal generic args through the same mapper policy.
    fn map_nominal_ty(&mut self, ty: &NominalTy) -> InferNominalTy {
        InferNominalTy {
            def: ty.def,
            args: ty
                .args
                .iter()
                .map(|arg| self.map_generic_arg(arg))
                .collect(),
        }
    }

    /// Convert opaque-bound generic args through the same mapper policy.
    fn map_opaque_bound(&mut self, bound: &OpaqueTraitBound) -> InferOpaqueTraitBound {
        InferOpaqueTraitBound {
            trait_ref: bound.trait_ref,
            args: bound
                .args
                .iter()
                .map(|arg| self.map_generic_arg(arg))
                .collect(),
        }
    }

    /// Convert one generic arg, recursing into type-bearing positions.
    fn map_generic_arg(&mut self, arg: &GenericArg) -> InferGenericArg {
        match arg {
            GenericArg::Type(ty) => InferGenericArg::Type(Box::new(self.map_ty(ty))),
            GenericArg::Lifetime(lifetime) => InferGenericArg::Lifetime(lifetime.clone()),
            GenericArg::Const(value) => InferGenericArg::Const(value.clone()),
            GenericArg::FnTraitArgs { params, ret } => InferGenericArg::FnTraitArgs {
                params: params.iter().map(|param| self.map_ty(param)).collect(),
                ret: Box::new(self.map_ty(ret)),
            },
            GenericArg::AssocType { name, ty } => InferGenericArg::AssocType {
                name: name.clone(),
                ty: ty.as_deref().map(|ty| Box::new(self.map_ty(ty))),
            },
            GenericArg::Unsupported(text) => InferGenericArg::Unsupported(text.clone()),
        }
    }

    /// Decide what a resolved `Ty::Unknown` means for this mapper.
    fn map_unknown_ty(&mut self) -> InferTy {
        InferTy::Unknown
    }
}

pub(crate) struct PlainTyToInferMapper;

impl TyToInferMapper for PlainTyToInferMapper {}

/// Projects written type syntax through a resolved type shape.
pub trait TypeRefInferenceProjector {
    /// Replace syntax markers such as `_` or a bound type param before walking children.
    fn replace_written_ty(&mut self, _written_ty: &TypeRef) -> Option<InferTy> {
        None
    }

    /// Project a written type through a resolved fallback, preserving policy replacements.
    fn project_ty(&mut self, written_ty: &TypeRef, resolved_ty: &Ty) -> InferTy {
        if let Some(ty) = self.replace_written_ty(written_ty) {
            return ty;
        }

        match (written_ty, resolved_ty) {
            (TypeRef::Unit, Ty::Unit) => InferTy::Unit,
            (TypeRef::Never, Ty::Never) => InferTy::Never,
            (TypeRef::Tuple(written_fields), Ty::Tuple(resolved_fields))
                if written_fields.len() == resolved_fields.len() =>
            {
                InferTy::Tuple(
                    written_fields
                        .iter()
                        .zip(resolved_fields)
                        .map(|(written_field, resolved_field)| {
                            self.project_ty(written_field, resolved_field)
                        })
                        .collect(),
                )
            }
            (
                TypeRef::Array {
                    inner: written_inner,
                    len: written_len,
                },
                Ty::Array {
                    inner: resolved_inner,
                    len: resolved_len,
                },
            ) if written_len == resolved_len => InferTy::Array {
                inner: Box::new(self.project_ty(written_inner, resolved_inner)),
                len: written_len.clone(),
            },
            (TypeRef::Slice(written_inner), Ty::Slice(resolved_inner)) => {
                InferTy::Slice(Box::new(self.project_ty(written_inner, resolved_inner)))
            }
            (
                TypeRef::Reference {
                    mutability,
                    inner: written_inner,
                    ..
                },
                Ty::Reference {
                    mutability: resolved_mutability,
                    inner: resolved_inner,
                },
            ) if *mutability == *resolved_mutability => InferTy::Reference {
                mutability: *resolved_mutability,
                inner: Box::new(self.project_ty(written_inner, resolved_inner)),
            },
            (
                TypeRef::Reference {
                    mutability,
                    inner: written_inner,
                    ..
                },
                Ty::Unknown,
            ) => InferTy::Reference {
                mutability: *mutability,
                inner: Box::new(self.project_ty(written_inner, &Ty::Unknown)),
            },
            (TypeRef::Path(path), Ty::Nominal(ty)) => self
                .project_nominal_ty(path, ty)
                .map(InferTy::Nominal)
                .unwrap_or_else(|| InferTy::from_ty(resolved_ty)),
            (TypeRef::Path(path), Ty::SelfTy(ty)) => self
                .project_nominal_ty(path, ty)
                .map(InferTy::SelfTy)
                .unwrap_or_else(|| InferTy::from_ty(resolved_ty)),
            _ => InferTy::from_ty(resolved_ty),
        }
    }

    /// Project path generic args onto an already-resolved nominal type.
    fn project_nominal_ty(&mut self, path: &TypePath, ty: &NominalTy) -> Option<InferNominalTy> {
        let segment = path.segments.last()?;
        if segment.args.len() != ty.args.len() {
            return None;
        }

        Some(InferNominalTy {
            def: ty.def,
            args: segment
                .args
                .iter()
                .zip(&ty.args)
                .map(|(written_arg, resolved_arg)| {
                    self.project_generic_arg(written_arg, resolved_arg)
                })
                .collect(),
        })
    }

    /// Project one written generic arg through its resolved fallback.
    fn project_generic_arg(
        &mut self,
        written_arg: &ItemGenericArg,
        resolved_arg: &GenericArg,
    ) -> InferGenericArg {
        match (written_arg, resolved_arg) {
            (ItemGenericArg::Type(written_ty), GenericArg::Type(resolved_ty)) => {
                InferGenericArg::Type(Box::new(self.project_ty(written_ty, resolved_ty)))
            }
            (
                ItemGenericArg::FnTraitArgs {
                    params: written_params,
                    ret,
                },
                GenericArg::FnTraitArgs {
                    params: resolved_params,
                    ret: resolved_ret,
                },
            ) if written_params.len() == resolved_params.len() => InferGenericArg::FnTraitArgs {
                params: written_params
                    .iter()
                    .zip(resolved_params)
                    .map(|(written_param, resolved_param)| {
                        self.project_ty(written_param, resolved_param)
                    })
                    .collect(),
                ret: Box::new(self.project_ty(ret, resolved_ret)),
            },
            (
                ItemGenericArg::AssocType {
                    name: written_name,
                    ty: Some(written_ty),
                },
                GenericArg::AssocType {
                    name: resolved_name,
                    ty: Some(resolved_ty),
                },
            ) if written_name == resolved_name => InferGenericArg::AssocType {
                name: written_name.clone(),
                ty: Some(Box::new(self.project_ty(written_ty, resolved_ty))),
            },
            _ => InferGenericArg::from_arg(resolved_arg),
        }
    }
}
