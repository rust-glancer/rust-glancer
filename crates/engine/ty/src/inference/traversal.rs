//! Shared inference traversal and structural helpers.
//!
//! Inference has several places that need the same recursive walk through `Ty` and `GenericArg`.
//! This module owns that repetition, plus the private structural checks that decide whether table
//! evidence can flow through nested children.

use super::var::{InferVarId, InferVarKind};
use crate::{
    AdtTy, AliasTy, ConstValue, FnDefTy, GenericArg, Lifetime, OpaqueTy, ProjectionTy, Ty,
};

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
            Ty::Reference {
                lifetime,
                mutability,
                inner,
            } => self.fold_reference(*lifetime, *mutability, inner),
            Ty::RawPointer { mutability, inner } => Ty::RawPointer {
                mutability: *mutability,
                inner: Box::new(self.fold_ty(inner)),
            },
            Ty::FnPointer { params, ret } => Ty::FnPointer {
                params: params.iter().map(|param| self.fold_ty(param)).collect(),
                ret: Box::new(self.fold_ty(ret)),
            },
            Ty::Closure(id) => Ty::Closure(*id),
            Ty::FnDef(function) => Ty::FnDef(FnDefTy {
                def: function.def,
                args: function
                    .args
                    .iter()
                    .map(|arg| self.fold_generic_arg(arg))
                    .collect(),
            }),
            Ty::Adt(ty) => Ty::Adt(self.fold_adt_ty(ty)),
            Ty::Param(param) => Ty::Param(*param),
            Ty::Alias(alias) => Ty::Alias(match alias {
                AliasTy::Projection(alias) => AliasTy::Projection(ProjectionTy {
                    associated_ty: alias.associated_ty,
                    args: alias
                        .args
                        .iter()
                        .map(|arg| self.fold_generic_arg(arg))
                        .collect(),
                }),
                AliasTy::Opaque(alias) => AliasTy::Opaque(OpaqueTy {
                    opaque: alias.opaque,
                    args: alias
                        .args
                        .iter()
                        .map(|arg| self.fold_generic_arg(arg))
                        .collect(),
                }),
            }),
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
    fn fold_array(&mut self, inner: &Ty, len: &ConstValue) -> Ty {
        Ty::Array {
            inner: Box::new(self.fold_ty(inner)),
            len: *len,
        }
    }

    /// Rebuild a slice after folding its element type.
    fn fold_slice(&mut self, inner: &Ty) -> Ty {
        Ty::Slice(Box::new(self.fold_ty(inner)))
    }

    /// Rebuild a reference after folding its inner type.
    fn fold_reference(
        &mut self,
        lifetime: Lifetime,
        mutability: crate::Mutability,
        inner: &Ty,
    ) -> Ty {
        Ty::Reference {
            lifetime,
            mutability,
            inner: Box::new(self.fold_ty(inner)),
        }
    }

    /// Fold generic args inside one nominal type.
    fn fold_adt_ty(&mut self, ty: &AdtTy) -> AdtTy {
        AdtTy {
            def: ty.def,
            args: ty
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
            GenericArg::Lifetime(lifetime) => GenericArg::Lifetime(*lifetime),
            GenericArg::Const(value) => GenericArg::Const(*value),
        }
    }
}

/// Return whether a type contains a specific inference variable.
pub(super) fn ty_contains_var(ty: &Ty, needle: InferVarId) -> bool {
    match ty {
        Ty::InferVar { id, .. } => *id == needle,
        Ty::Tuple(fields) => fields.iter().any(|field| ty_contains_var(field, needle)),
        Ty::Array { inner, .. }
        | Ty::Slice(inner)
        | Ty::Reference { inner, .. }
        | Ty::RawPointer { inner, .. } => ty_contains_var(inner, needle),
        Ty::FnPointer { params, ret } => {
            params.iter().any(|param| ty_contains_var(param, needle))
                || ty_contains_var(ret, needle)
        }
        Ty::Adt(ty) => ty
            .args
            .iter()
            .any(|arg| generic_arg_contains_var(arg, needle)),
        Ty::Alias(alias) => match alias {
            AliasTy::Projection(alias) => alias
                .args
                .iter()
                .any(|arg| generic_arg_contains_var(arg, needle)),
            AliasTy::Opaque(alias) => alias
                .args
                .iter()
                .any(|arg| generic_arg_contains_var(arg, needle)),
        },
        Ty::FnDef(function) => function
            .args
            .iter()
            .any(|arg| generic_arg_contains_var(arg, needle)),
        Ty::Unit | Ty::Never | Ty::Primitive(_) | Ty::Closure(_) | Ty::Param(_) | Ty::Unknown => {
            false
        }
    }
}

fn generic_arg_contains_var(arg: &GenericArg, needle: InferVarId) -> bool {
    match arg {
        GenericArg::Type(ty) => ty_contains_var(ty, needle),
        GenericArg::Lifetime(_) | GenericArg::Const(_) => false,
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
        | (Ty::Slice(_), Ty::Slice(_))
        | (Ty::Unknown, Ty::Unknown) => true,
        (Ty::Primitive(lhs), Ty::Primitive(rhs)) => lhs == rhs,
        (Ty::Closure(lhs), Ty::Closure(rhs)) => lhs == rhs,
        (Ty::FnDef(lhs), Ty::FnDef(rhs)) => lhs.def == rhs.def && lhs.args.len() == rhs.args.len(),
        (Ty::Param(lhs), Ty::Param(rhs)) => lhs == rhs,
        (Ty::Alias(lhs), Ty::Alias(rhs)) => {
            lhs.same_definition(rhs) && lhs.args().len() == rhs.args().len()
        }
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
        (
            Ty::RawPointer {
                mutability: lhs, ..
            },
            Ty::RawPointer {
                mutability: rhs, ..
            },
        ) => lhs == rhs,
        (Ty::FnPointer { params: lhs, .. }, Ty::FnPointer { params: rhs, .. }) => {
            lhs.len() == rhs.len()
        }
        (Ty::Adt(lhs), Ty::Adt(rhs)) => same_adt_shape(lhs, rhs),
        _ => false,
    }
}

fn same_adt_shape(lhs: &AdtTy, rhs: &AdtTy) -> bool {
    lhs.def == rhs.def && lhs.args.len() == rhs.args.len()
}

/// Return whether two generic args have the same child layout for inference.
pub(super) fn same_generic_arg_shape(lhs: &GenericArg, rhs: &GenericArg) -> bool {
    match (lhs, rhs) {
        (GenericArg::Type(_), GenericArg::Type(_)) => true,
        (GenericArg::Lifetime(lhs), GenericArg::Lifetime(rhs)) => lhs == rhs,
        (GenericArg::Const(lhs), GenericArg::Const(rhs)) => lhs == rhs,
        _ => false,
    }
}
