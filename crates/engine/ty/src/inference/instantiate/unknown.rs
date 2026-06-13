use rg_std::UniqueVec;

use super::super::{
    model::{InferGenericArg, InferNominalTy, InferOpaqueTraitBound, InferTy},
    table::InferenceTable,
};
use crate::{GenericArg, NominalTy, OpaqueTraitBound, Ty};

/// Instantiates unknowns nested inside a known return shape.
///
/// ```text
/// impl<T> Vec<T> { fn new() -> Self }
/// Vec::new()          // selected Self: Vec<unknown>, resolved return: Vec<unknown>
///                     // inference return: Vec<?T>
///
/// missing()           // resolved return: <unknown>
///                     // inference return: <unknown>
/// ```
pub struct UnknownTypeInstantiationBuilder<'table> {
    table: &'table mut InferenceTable,
    used_type_vars: bool,
}

impl<'table> UnknownTypeInstantiationBuilder<'table> {
    pub fn new(table: &'table mut InferenceTable) -> Self {
        Self {
            table,
            used_type_vars: false,
        }
    }

    pub fn used_type_vars(&self) -> bool {
        self.used_type_vars
    }

    pub fn ty_from_ty(&mut self, ty: &Ty) -> InferTy {
        // We don't instantiate root unknown.
        if matches!(ty, Ty::Unknown) {
            return InferTy::Unknown;
        }

        // For whatever unknowns exist inside of `Ty`, replace them with `?T`.
        self.ty_from_ty_inner(ty)
    }

    // Within a `Ty`, replace every `Ty::Unknown` with a new infer type variable.
    fn ty_from_ty_inner(&mut self, ty: &Ty) -> InferTy {
        match ty {
            Ty::Unit => InferTy::Unit,
            Ty::Never => InferTy::Never,
            Ty::Primitive(primitive) => InferTy::Primitive(*primitive),
            Ty::Tuple(fields) => InferTy::Tuple(
                fields
                    .iter()
                    .map(|field| self.ty_from_ty_inner(field))
                    .collect(),
            ),
            Ty::Array { inner, len } => InferTy::Array {
                inner: Box::new(self.ty_from_ty_inner(inner)),
                len: len.clone(),
            },
            Ty::Slice(inner) => InferTy::Slice(Box::new(self.ty_from_ty_inner(inner))),
            Ty::Reference { mutability, inner } => InferTy::Reference {
                mutability: *mutability,
                inner: Box::new(self.ty_from_ty_inner(inner)),
            },
            Ty::Opaque { bounds } => InferTy::Opaque {
                bounds: bounds
                    .iter()
                    .map(|bound| self.opaque_bound_from_bound(bound))
                    .collect::<UniqueVec<_>>(),
            },
            Ty::Syntax(ty) => InferTy::Syntax(Box::new(ty.clone())),
            Ty::Nominal(ty) => InferTy::Nominal(self.nominal_ty_from_ty(ty)),
            Ty::SelfTy(ty) => InferTy::SelfTy(self.nominal_ty_from_ty(ty)),
            Ty::Unknown => {
                self.used_type_vars = true;
                self.table.new_type_var()
            }
        }
    }

    fn nominal_ty_from_ty(&mut self, ty: &NominalTy) -> InferNominalTy {
        InferNominalTy {
            def: ty.def,
            args: ty
                .args
                .iter()
                .map(|arg| self.generic_arg_from_arg(arg))
                .collect(),
        }
    }

    fn opaque_bound_from_bound(&mut self, bound: &OpaqueTraitBound) -> InferOpaqueTraitBound {
        InferOpaqueTraitBound {
            trait_ref: bound.trait_ref,
            args: bound
                .args
                .iter()
                .map(|arg| self.generic_arg_from_arg(arg))
                .collect(),
        }
    }

    fn generic_arg_from_arg(&mut self, arg: &GenericArg) -> InferGenericArg {
        match arg {
            GenericArg::Type(ty) => InferGenericArg::Type(Box::new(self.ty_from_ty_inner(ty))),
            GenericArg::Lifetime(lifetime) => InferGenericArg::Lifetime(lifetime.clone()),
            GenericArg::Const(value) => InferGenericArg::Const(value.clone()),
            GenericArg::FnTraitArgs { params, ret } => InferGenericArg::FnTraitArgs {
                params: params
                    .iter()
                    .map(|param| self.ty_from_ty_inner(param))
                    .collect(),
                ret: Box::new(self.ty_from_ty_inner(ret)),
            },
            GenericArg::AssocType { name, ty } => InferGenericArg::AssocType {
                name: name.clone(),
                ty: ty.as_ref().map(|ty| Box::new(self.ty_from_ty_inner(ty))),
            },
            GenericArg::Unsupported(text) => InferGenericArg::Unsupported(text.clone()),
        }
    }
}
