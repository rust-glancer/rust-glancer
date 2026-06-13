use rg_ir_model::items::{GenericArg as ItemGenericArg, GenericParams, TypePath, TypeRef};
use rg_text::Name;

use super::super::{
    model::{InferGenericArg, InferNominalTy, InferTy},
    table::InferenceTable,
};
use crate::{GenericArg, NominalTy, Ty};

/// Walks written type syntax together with the resolved type shape.
trait TypeRefTyInstantiator {
    /// Return a policy-specific type before the shared shape walk.
    fn instantiate_special_ty(&mut self, written_ty: &TypeRef, resolved_ty: &Ty)
    -> Option<InferTy>;

    fn ty_from_type_ref(&mut self, written_ty: &TypeRef, resolved_ty: &Ty) -> InferTy {
        if let Some(ty) = self.instantiate_special_ty(written_ty, resolved_ty) {
            return ty;
        }

        match (written_ty, resolved_ty) {
            // Exact builtin shapes do not contain nested type positions.
            (TypeRef::Unit, Ty::Unit) => InferTy::Unit,
            (TypeRef::Never, Ty::Never) => InferTy::Never,

            // Structural containers recurse into positions where inference syntax may appear.
            (TypeRef::Tuple(written_fields), Ty::Tuple(resolved_fields))
                if written_fields.len() == resolved_fields.len() =>
            {
                InferTy::Tuple(
                    written_fields
                        .iter()
                        .zip(resolved_fields)
                        .map(|(written_field, resolved_field)| {
                            self.ty_from_type_ref(written_field, resolved_field)
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
                inner: Box::new(self.ty_from_type_ref(written_inner, resolved_inner)),
                len: written_len.clone(),
            },
            (TypeRef::Slice(written_inner), Ty::Slice(resolved_inner)) => InferTy::Slice(Box::new(
                self.ty_from_type_ref(written_inner, resolved_inner),
            )),

            // References may hide inference positions after `Ty` has collapsed `&Unknown`.
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
                inner: Box::new(self.ty_from_type_ref(written_inner, resolved_inner)),
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
                inner: Box::new(self.ty_from_type_ref(written_inner, &Ty::Unknown)),
            },

            // Nominal paths expose generic args where the policy may find variables.
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
                .map(|(written_arg, resolved_arg)| {
                    self.generic_arg_from_type_ref_arg(written_arg, resolved_arg)
                })
                .collect(),
        })
    }

    fn generic_arg_from_type_ref_arg(
        &mut self,
        written_arg: &ItemGenericArg,
        resolved_arg: &GenericArg,
    ) -> InferGenericArg {
        match (written_arg, resolved_arg) {
            // Type args are direct type positions.
            (ItemGenericArg::Type(written_ty), GenericArg::Type(resolved_ty)) => {
                InferGenericArg::Type(Box::new(self.ty_from_type_ref(written_ty, resolved_ty)))
            }

            // Parenthesized `Fn*` args expose parameter and return type positions.
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
                        self.ty_from_type_ref(written_param, resolved_param)
                    })
                    .collect(),
                ret: Box::new(self.ty_from_type_ref(ret, resolved_ret)),
            },

            // Associated type equalities expose one named type position.
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
                ty: Some(Box::new(self.ty_from_type_ref(written_ty, resolved_ty))),
            },

            _ => InferGenericArg::from_arg(resolved_arg),
        }
    }
}

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
        self.ty_from_type_ref(ret_ty, resolved_ty)
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
}

impl TypeRefTyInstantiator for GenericReturnInstantiationBuilder<'_> {
    /// Instantiate return type params such as `T` in `fn make<T>() -> T`.
    fn instantiate_special_ty(
        &mut self,
        written_ty: &TypeRef,
        _resolved_ty: &Ty,
    ) -> Option<InferTy> {
        self.var_for_plain_type_param(written_ty)
    }
}

/// Instantiates explicit `_` type args as variables.
///
/// ```text
/// make::<_>()         // resolved arg: <unknown>
///                     // inference arg: ?T
///
/// make::<Vec<_>>()    // resolved arg: Vec<unknown>
///                     // inference arg: Vec<?T>
/// ```
pub struct ExplicitTypeArgInstantiationBuilder<'table> {
    table: &'table mut InferenceTable,
    used_type_vars: bool,
}

impl<'table> ExplicitTypeArgInstantiationBuilder<'table> {
    pub fn new(table: &'table mut InferenceTable) -> Self {
        Self {
            table,
            used_type_vars: false,
        }
    }

    pub fn used_type_vars(&self) -> bool {
        self.used_type_vars
    }

    /// Convert one explicit type arg into an inference-aware type.
    pub fn ty_from_arg(&mut self, arg_ty: &TypeRef, resolved_ty: &Ty) -> InferTy {
        self.ty_from_type_ref(arg_ty, resolved_ty)
    }
}

impl TypeRefTyInstantiator for ExplicitTypeArgInstantiationBuilder<'_> {
    /// Instantiate written `_` slots in explicit args such as `make::<Vec<_>>()`.
    fn instantiate_special_ty(
        &mut self,
        written_ty: &TypeRef,
        _resolved_ty: &Ty,
    ) -> Option<InferTy> {
        if matches!(written_ty, TypeRef::Infer) {
            self.used_type_vars = true;
            return Some(self.table.new_type_var());
        }

        None
    }
}
