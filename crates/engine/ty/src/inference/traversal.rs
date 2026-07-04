//! Shared inference traversal and structural helpers.
//!
//! Inference has several places that need the same recursive walk through `Ty`, `GenericArg`, and
//! written `TypeRef` shapes. This module owns that repetition, plus the private structural checks
//! that decide whether table evidence can flow through nested children.

use rg_ir_model::items::{GenericArg as ItemGenericArg, TypePath, TypeRef};

use super::var::{InferVarId, InferVarKind};
use crate::{GenericArg, NominalTy, OpaqueTraitBound, Ty};

/// Shared traversal over an inference-capable `Ty` tree.
///
/// Inference has several policies that all need to walk the same recursive shape:
/// canonicalization follows solved variables, finalization erases variables, and wildcard
/// instantiation replaces nested `Unknown` nodes with fresh variables. Keep that shape walk here
/// so each caller only describes the policy points that are actually different.
pub(super) trait InferenceTyFolder {
    /// Fold one type, recursing through type-bearing children.
    fn fold_ty(&mut self, ty: &Ty) -> Ty {
        match ty {
            Ty::InferVar { kind, id } => self.fold_infer_var(*id, *kind),
            Ty::Unit => Ty::Unit,
            Ty::Never => Ty::Never,
            Ty::Primitive(primitive) => Ty::Primitive(*primitive),
            Ty::Tuple(fields) => self.fold_tuple(fields),
            Ty::Array { inner, len } => self.fold_array(inner, len),
            Ty::Slice(inner) => self.fold_slice(inner),
            Ty::Reference { mutability, inner } => self.fold_reference(*mutability, inner),
            Ty::Opaque { bounds } => self.fold_opaque(bounds),
            Ty::Closure(id) => Ty::Closure(*id),
            Ty::FunctionItem(function) => Ty::FunctionItem(*function),
            Ty::Syntax(ty) => Ty::Syntax(ty.clone()),
            Ty::Nominal(ty) => Ty::Nominal(self.fold_nominal_ty(ty)),
            Ty::SelfTy(ty) => Ty::SelfTy(self.fold_nominal_ty(ty)),
            Ty::Unknown => self.fold_unknown(),
        }
    }

    /// Fold an inference variable. The default keeps it as-is.
    fn fold_infer_var(&mut self, id: InferVarId, kind: InferVarKind) -> Ty {
        Ty::var_for_kind(kind, id)
    }

    /// Fold `Unknown`. The default keeps it as-is.
    fn fold_unknown(&mut self) -> Ty {
        Ty::Unknown
    }

    /// Rebuild a tuple after folding its children.
    fn fold_tuple(&mut self, fields: &[Ty]) -> Ty {
        Ty::Tuple(fields.iter().map(|field| self.fold_ty(field)).collect())
    }

    /// Rebuild an array after folding its element type.
    fn fold_array(&mut self, inner: &Ty, len: &Option<String>) -> Ty {
        Ty::Array {
            inner: Box::new(self.fold_ty(inner)),
            len: len.clone(),
        }
    }

    /// Rebuild a slice after folding its element type.
    fn fold_slice(&mut self, inner: &Ty) -> Ty {
        Ty::Slice(Box::new(self.fold_ty(inner)))
    }

    /// Rebuild a reference after folding its inner type.
    fn fold_reference(&mut self, mutability: crate::Mutability, inner: &Ty) -> Ty {
        Ty::Reference {
            mutability,
            inner: Box::new(self.fold_ty(inner)),
        }
    }

    /// Rebuild an opaque type after folding all bound args.
    fn fold_opaque(&mut self, bounds: &rg_std::UniqueVec<OpaqueTraitBound>) -> Ty {
        Ty::Opaque {
            bounds: bounds
                .iter()
                .map(|bound| self.fold_opaque_bound(bound))
                .collect(),
        }
    }

    /// Fold generic args inside one nominal type.
    fn fold_nominal_ty(&mut self, ty: &NominalTy) -> NominalTy {
        NominalTy {
            def: ty.def,
            args: ty
                .args
                .iter()
                .map(|arg| self.fold_generic_arg(arg))
                .collect(),
        }
    }

    /// Fold generic args inside one opaque bound.
    fn fold_opaque_bound(&mut self, bound: &OpaqueTraitBound) -> OpaqueTraitBound {
        OpaqueTraitBound {
            trait_ref: bound.trait_ref,
            args: bound
                .args
                .iter()
                .map(|arg| self.fold_generic_arg(arg))
                .collect(),
        }
    }

    /// Fold one generic argument, recursing through type-bearing positions.
    fn fold_generic_arg(&mut self, arg: &GenericArg) -> GenericArg {
        match arg {
            GenericArg::Type(ty) => GenericArg::Type(Box::new(self.fold_ty(ty))),
            GenericArg::Lifetime(lifetime) => GenericArg::Lifetime(lifetime.clone()),
            GenericArg::Const(value) => GenericArg::Const(value.clone()),
            GenericArg::FnTraitArgs { params, ret } => GenericArg::FnTraitArgs {
                params: params.iter().map(|param| self.fold_ty(param)).collect(),
                ret: Box::new(self.fold_ty(ret)),
            },
            GenericArg::AssocType { name, ty } => GenericArg::AssocType {
                name: name.clone(),
                ty: ty.as_deref().map(|ty| Box::new(self.fold_ty(ty))),
            },
            GenericArg::Unsupported(text) => GenericArg::Unsupported(text.clone()),
        }
    }
}

/// Return whether a type contains a specific inference variable.
pub(super) fn ty_contains_var(ty: &Ty, needle: InferVarId) -> bool {
    match ty {
        Ty::InferVar { id, .. } => *id == needle,
        Ty::Tuple(fields) => fields.iter().any(|field| ty_contains_var(field, needle)),
        Ty::Array { inner, .. } | Ty::Slice(inner) | Ty::Reference { inner, .. } => {
            ty_contains_var(inner, needle)
        }
        Ty::Opaque { bounds } => bounds.iter().any(|bound| {
            bound
                .args
                .iter()
                .any(|arg| generic_arg_contains_var(arg, needle))
        }),
        Ty::Nominal(ty) | Ty::SelfTy(ty) => ty
            .args
            .iter()
            .any(|arg| generic_arg_contains_var(arg, needle)),
        Ty::Unit
        | Ty::Never
        | Ty::Primitive(_)
        | Ty::Closure(_)
        | Ty::FunctionItem(_)
        | Ty::Syntax(_)
        | Ty::Unknown => false,
    }
}

fn generic_arg_contains_var(arg: &GenericArg, needle: InferVarId) -> bool {
    match arg {
        GenericArg::Type(ty) => ty_contains_var(ty, needle),
        GenericArg::FnTraitArgs { params, ret } => {
            params.iter().any(|param| ty_contains_var(param, needle))
                || ty_contains_var(ret, needle)
        }
        GenericArg::AssocType { ty, .. } => {
            ty.as_deref().is_some_and(|ty| ty_contains_var(ty, needle))
        }
        GenericArg::Lifetime(_) | GenericArg::Const(_) | GenericArg::Unsupported(_) => false,
    }
}

/// Return whether two types can use the same inference structural branch.
pub(super) fn same_ty_shape(lhs: &Ty, rhs: &Ty) -> bool {
    match (lhs, rhs) {
        (Ty::InferVar { kind: lhs_kind, .. }, Ty::InferVar { kind: rhs_kind, .. }) => {
            lhs_kind == rhs_kind
        }
        (Ty::Unit, Ty::Unit)
        | (Ty::Never, Ty::Never)
        | (Ty::Opaque { .. }, Ty::Opaque { .. })
        | (Ty::Slice(_), Ty::Slice(_))
        | (Ty::Unknown, Ty::Unknown) => true,
        (Ty::Primitive(lhs), Ty::Primitive(rhs)) => lhs == rhs,
        (Ty::Closure(lhs), Ty::Closure(rhs)) => lhs == rhs,
        (Ty::FunctionItem(lhs), Ty::FunctionItem(rhs)) => lhs == rhs,
        (Ty::Tuple(lhs), Ty::Tuple(rhs)) => lhs.len() == rhs.len(),
        (Ty::Array { len: lhs_len, .. }, Ty::Array { len: rhs_len, .. }) => lhs_len == rhs_len,
        (
            Ty::Reference {
                mutability: lhs_mutability,
                ..
            },
            Ty::Reference {
                mutability: rhs_mutability,
                ..
            },
        ) => lhs_mutability == rhs_mutability,
        (Ty::Syntax(lhs), Ty::Syntax(rhs)) => lhs == rhs,
        (Ty::Nominal(lhs), Ty::Nominal(rhs)) | (Ty::SelfTy(lhs), Ty::SelfTy(rhs)) => {
            same_nominal_shape(lhs, rhs)
        }
        _ => false,
    }
}

fn same_nominal_shape(lhs: &NominalTy, rhs: &NominalTy) -> bool {
    lhs.def == rhs.def && lhs.args.len() == rhs.args.len()
}

/// Return whether two opaque bounds can pass evidence through their generic args.
pub(super) fn same_opaque_trait_shape(lhs: &OpaqueTraitBound, rhs: &OpaqueTraitBound) -> bool {
    lhs.trait_ref == rhs.trait_ref && lhs.args.len() == rhs.args.len()
}

/// Return whether two generic args have the same child layout for inference.
pub(super) fn same_generic_arg_shape(lhs: &GenericArg, rhs: &GenericArg) -> bool {
    match (lhs, rhs) {
        (GenericArg::Type(_), GenericArg::Type(_)) => true,
        (GenericArg::Lifetime(lhs), GenericArg::Lifetime(rhs)) => lhs == rhs,
        (GenericArg::Const(lhs), GenericArg::Const(rhs)) => lhs == rhs,
        (
            GenericArg::FnTraitArgs { params: lhs, .. },
            GenericArg::FnTraitArgs { params: rhs, .. },
        ) => lhs.len() == rhs.len(),
        (
            GenericArg::AssocType {
                name: lhs_name,
                ty: lhs_ty,
            },
            GenericArg::AssocType {
                name: rhs_name,
                ty: rhs_ty,
            },
        ) => lhs_name == rhs_name && lhs_ty.is_some() == rhs_ty.is_some(),
        (GenericArg::Unsupported(lhs), GenericArg::Unsupported(rhs)) => lhs == rhs,
        _ => false,
    }
}

/// Projects written type syntax through a resolved type shape.
pub(super) trait TypeRefInferenceProjector {
    /// Replace syntax markers such as `_` or a bound type param before walking children.
    fn replace_written_ty(&mut self, _written_ty: &TypeRef) -> Option<Ty> {
        None
    }

    /// Project a written type through a resolved fallback, preserving policy replacements.
    fn project_ty(&mut self, written_ty: &TypeRef, resolved_ty: &Ty) -> Ty {
        if let Some(ty) = self.replace_written_ty(written_ty) {
            return ty;
        }

        match (written_ty, resolved_ty) {
            (TypeRef::Unit, Ty::Unit) => Ty::Unit,
            (TypeRef::Never, Ty::Never) => Ty::Never,
            (TypeRef::Tuple(written_fields), Ty::Tuple(resolved_fields))
                if written_fields.len() == resolved_fields.len() =>
            {
                Ty::Tuple(
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
            ) if written_len == resolved_len => Ty::Array {
                inner: Box::new(self.project_ty(written_inner, resolved_inner)),
                len: written_len.clone(),
            },
            (TypeRef::Slice(written_inner), Ty::Slice(resolved_inner)) => {
                Ty::Slice(Box::new(self.project_ty(written_inner, resolved_inner)))
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
            ) if *mutability == *resolved_mutability => Ty::Reference {
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
            ) => Ty::Reference {
                mutability: *mutability,
                inner: Box::new(self.project_ty(written_inner, &Ty::Unknown)),
            },
            (TypeRef::Path(path), Ty::Nominal(ty)) => self
                .project_nominal_ty(path, ty)
                .map(Ty::Nominal)
                .unwrap_or_else(|| resolved_ty.clone()),
            (TypeRef::Path(path), Ty::SelfTy(ty)) => self
                .project_nominal_ty(path, ty)
                .map(Ty::SelfTy)
                .unwrap_or_else(|| resolved_ty.clone()),
            _ => resolved_ty.clone(),
        }
    }

    /// Project path generic args onto an already-resolved nominal type.
    fn project_nominal_ty(&mut self, path: &TypePath, ty: &NominalTy) -> Option<NominalTy> {
        let segment = path.segments.last()?;
        if segment.args.len() != ty.args.len() {
            return None;
        }

        Some(NominalTy {
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
    ) -> GenericArg {
        match (written_arg, resolved_arg) {
            (ItemGenericArg::Type(written_ty), GenericArg::Type(resolved_ty)) => {
                GenericArg::Type(Box::new(self.project_ty(written_ty, resolved_ty)))
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
            ) if written_params.len() == resolved_params.len() => GenericArg::FnTraitArgs {
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
            ) if written_name == resolved_name => GenericArg::AssocType {
                name: written_name.clone(),
                ty: Some(Box::new(self.project_ty(written_ty, resolved_ty))),
            },
            _ => resolved_arg.clone(),
        }
    }
}
