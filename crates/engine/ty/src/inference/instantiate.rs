use rg_ir_model::items::{
    GenericArg as ItemGenericArg, GenericParams, Mutability, TypePath, TypeRef,
};
use rg_std::UniqueVec;
use rg_text::Name;

use super::{
    model::{InferGenericArg, InferNominalTy, InferOpaqueTraitBound, InferTy},
    table::InferenceTable,
};
use crate::{GenericArg, NominalTy, OpaqueTraitBound, RefMutability, Ty};

/// Instantiates function type params as variables inside a projected call return.
///
/// ```text
/// fn id<T>(value: T) -> T
/// id(missing())       // resolved return: <unknown>, declared return: T
///                     // inference return: ?T
///
/// fn make_vec<T>() -> Vec<T>
/// make_vec()          // resolved return: Vec<unknown>, declared return: Vec<T>
///                     // inference return: Vec<?T>
/// ```
pub struct GenericReturnInstantiationBuilder<'table> {
    table: &'table mut InferenceTable,
    params: Vec<(Name, Option<InferTy>)>,
    used_type_vars: bool,
}

impl<'table> GenericReturnInstantiationBuilder<'table> {
    pub fn new(table: &'table mut InferenceTable, generics: &GenericParams) -> Self {
        Self {
            table,
            params: generics
                .types
                .iter()
                .map(|param| (param.name.clone(), None))
                .collect(),
            used_type_vars: false,
        }
    }

    pub fn used_type_vars(&self) -> bool {
        self.used_type_vars
    }

    pub fn ty_from_return(&mut self, ret_ty: &TypeRef, resolved_ty: &Ty) -> InferTy {
        if let Some(var) = self.var_for_plain_type_param(ret_ty) {
            return var;
        }

        match (ret_ty, resolved_ty) {
            // Exact builtin shapes do not contain type params to instantiate.
            (TypeRef::Unit, Ty::Unit) => InferTy::Unit,
            (TypeRef::Never, Ty::Never) => InferTy::Never,

            // Structural containers recurse into the child type positions.
            (TypeRef::Tuple(ret_fields), Ty::Tuple(resolved_fields))
                if ret_fields.len() == resolved_fields.len() =>
            {
                InferTy::Tuple(
                    ret_fields
                        .iter()
                        .zip(resolved_fields)
                        .map(|(ret_field, resolved_field)| {
                            self.ty_from_return(ret_field, resolved_field)
                        })
                        .collect(),
                )
            }
            (
                TypeRef::Array {
                    inner: ret_inner,
                    len: ret_len,
                },
                Ty::Array {
                    inner: resolved_inner,
                    len: resolved_len,
                },
            ) if ret_len == resolved_len => InferTy::Array {
                inner: Box::new(self.ty_from_return(ret_inner, resolved_inner)),
                len: ret_len.clone(),
            },
            (TypeRef::Slice(ret_inner), Ty::Slice(resolved_inner)) => {
                InferTy::Slice(Box::new(self.ty_from_return(ret_inner, resolved_inner)))
            }

            // References may hide `?T` even when the resolved shape has collapsed to unknown.
            (
                TypeRef::Reference {
                    mutability,
                    inner: ret_inner,
                    ..
                },
                Ty::Reference {
                    mutability: resolved_mutability,
                    inner: resolved_inner,
                },
            ) if Self::ref_mutability(*mutability) == *resolved_mutability => InferTy::Reference {
                mutability: *resolved_mutability,
                inner: Box::new(self.ty_from_return(ret_inner, resolved_inner)),
            },
            (
                TypeRef::Reference {
                    mutability,
                    inner: ret_inner,
                    ..
                },
                Ty::Unknown,
            ) => {
                // `Ty::reference` collapses `&Unknown` to `Unknown`; the declared return syntax
                // still lets inference keep `&?T` alive long enough for expected types to solve it.
                InferTy::Reference {
                    mutability: Self::ref_mutability(*mutability),
                    inner: Box::new(self.ty_from_return(ret_inner, &Ty::Unknown)),
                }
            }

            // Nominal paths expose generic args where return type params may sit.
            (TypeRef::Path(path), Ty::Nominal(ty)) => self
                .nominal_ty_from_path(path, ty)
                .map(InferTy::Nominal)
                .unwrap_or_else(|| InferTy::from_ty(resolved_ty)),
            (TypeRef::Path(path), Ty::SelfTy(ty)) => self
                .nominal_ty_from_path(path, ty)
                .map(InferTy::SelfTy)
                .unwrap_or_else(|| InferTy::from_ty(resolved_ty)),

            _ => InferTy::from_ty(resolved_ty),
        }
    }

    fn var_for_plain_type_param(&mut self, ret_ty: &TypeRef) -> Option<InferTy> {
        let name = ret_ty.type_param_name()?;
        let idx = self
            .params
            .iter()
            .position(|(param, _)| param.as_str() == name.as_str())?;

        if self.params[idx].1.is_none() {
            self.params[idx].1 = Some(self.table.new_type_var());
        }
        self.used_type_vars = true;
        self.params[idx].1.clone()
    }

    fn nominal_ty_from_path(&mut self, path: &TypePath, ty: &NominalTy) -> Option<InferNominalTy> {
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
                .map(|(ret_arg, resolved_arg)| self.generic_arg_from_return(ret_arg, resolved_arg))
                .collect(),
        })
    }

    fn generic_arg_from_return(
        &mut self,
        ret_arg: &ItemGenericArg,
        resolved_arg: &GenericArg,
    ) -> InferGenericArg {
        match (ret_arg, resolved_arg) {
            // Type args are direct type positions.
            (ItemGenericArg::Type(ret_ty), GenericArg::Type(resolved_ty)) => {
                InferGenericArg::Type(Box::new(self.ty_from_return(ret_ty, resolved_ty)))
            }

            // Parenthesized `Fn*` args expose parameter and return type positions.
            (
                ItemGenericArg::FnTraitArgs {
                    params: ret_params,
                    ret,
                },
                GenericArg::FnTraitArgs {
                    params: resolved_params,
                    ret: resolved_ret,
                },
            ) if ret_params.len() == resolved_params.len() => InferGenericArg::FnTraitArgs {
                params: ret_params
                    .iter()
                    .zip(resolved_params)
                    .map(|(ret_param, resolved_param)| {
                        self.ty_from_return(ret_param, resolved_param)
                    })
                    .collect(),
                ret: Box::new(self.ty_from_return(ret, resolved_ret)),
            },

            // Associated type equalities expose one named type position.
            (
                ItemGenericArg::AssocType {
                    name: ret_name,
                    ty: Some(ret_ty),
                },
                GenericArg::AssocType {
                    name: resolved_name,
                    ty: Some(resolved_ty),
                },
            ) if ret_name == resolved_name => InferGenericArg::AssocType {
                name: ret_name.clone(),
                ty: Some(Box::new(self.ty_from_return(ret_ty, resolved_ty))),
            },

            _ => InferGenericArg::from_arg(resolved_arg),
        }
    }

    fn ref_mutability(mutability: Mutability) -> RefMutability {
        match mutability {
            Mutability::Shared => RefMutability::Shared,
            Mutability::Mutable => RefMutability::Mutable,
        }
    }
}

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
        self.ty_from_ty_inner(ty, false)
    }

    fn ty_from_ty_inner(&mut self, ty: &Ty, instantiate_unknown: bool) -> InferTy {
        match ty {
            Ty::Unit => InferTy::Unit,
            Ty::Never => InferTy::Never,
            Ty::Primitive(primitive) => InferTy::Primitive(*primitive),
            Ty::Tuple(fields) => InferTy::Tuple(
                fields
                    .iter()
                    .map(|field| self.ty_from_ty_inner(field, true))
                    .collect(),
            ),
            Ty::Array { inner, len } => InferTy::Array {
                inner: Box::new(self.ty_from_ty_inner(inner, true)),
                len: len.clone(),
            },
            Ty::Slice(inner) => InferTy::Slice(Box::new(self.ty_from_ty_inner(inner, true))),
            Ty::Reference { mutability, inner } => InferTy::Reference {
                mutability: *mutability,
                inner: Box::new(self.ty_from_ty_inner(inner, true)),
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
            Ty::Unknown if instantiate_unknown => {
                self.used_type_vars = true;
                self.table.new_type_var()
            }
            Ty::Unknown => InferTy::Unknown,
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
            GenericArg::Type(ty) => {
                InferGenericArg::Type(Box::new(self.ty_from_ty_inner(ty, true)))
            }
            GenericArg::Lifetime(lifetime) => InferGenericArg::Lifetime(lifetime.clone()),
            GenericArg::Const(value) => InferGenericArg::Const(value.clone()),
            GenericArg::FnTraitArgs { params, ret } => InferGenericArg::FnTraitArgs {
                params: params
                    .iter()
                    .map(|param| self.ty_from_ty_inner(param, true))
                    .collect(),
                ret: Box::new(self.ty_from_ty_inner(ret, true)),
            },
            GenericArg::AssocType { name, ty } => InferGenericArg::AssocType {
                name: name.clone(),
                ty: ty
                    .as_ref()
                    .map(|ty| Box::new(self.ty_from_ty_inner(ty, true))),
            },
            GenericArg::Unsupported(text) => InferGenericArg::Unsupported(text.clone()),
        }
    }
}
