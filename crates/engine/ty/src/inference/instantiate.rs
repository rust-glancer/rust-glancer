use rg_ir_model::items::{
    GenericArg as ItemGenericArg, GenericParams, Mutability, TypePath, TypeRef,
};
use rg_text::Name;

use super::{
    model::{InferGenericArg, InferNominalTy, InferTy},
    table::InferenceTable,
};
use crate::{GenericArg, NominalTy, RefMutability, Ty};

/// Instantiates function type params as variables inside a projected call return.
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
