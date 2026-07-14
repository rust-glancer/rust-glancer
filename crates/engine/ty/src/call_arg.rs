use rg_ir_model::GenericParamRef;
use rg_semantic_ir::Generics;

use crate::{GenericArg, Substitution, Ty};

/// How call-site arguments line up with the selected callable's semantic parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallArgMapping {
    /// Written arguments begin at the first declared parameter.
    FunctionCall,
    /// Method syntax supplies the receiver separately, so written arguments begin after `self`.
    MethodCall,
}

impl CallArgMapping {
    fn first_param_idx(self) -> usize {
        match self {
            Self::FunctionCall => 0,
            Self::MethodCall => 1,
        }
    }
}

/// Infers function-owned parameter bindings from a canonical callable signature.
///
/// For `fn wrap<T>(value: Option<T>)`, an `Option<User>` argument contributes `T = User` by
/// matching the two semantic type shapes. Only parameters owned by this function are inferred;
/// inherited trait or impl parameters keep the bindings already selected by the receiver.
pub struct CallArgInference<'signature, 'arg> {
    generics: &'signature Generics<'signature>,
    params: &'signature [Ty],
    arg_tys: &'arg [Ty],
    arg_mapping: CallArgMapping,
    existing_subst: &'arg Substitution,
}

impl<'signature, 'arg> CallArgInference<'signature, 'arg> {
    pub fn new(
        generics: &'signature Generics<'signature>,
        params: &'signature [Ty],
        arg_tys: &'arg [Ty],
        arg_mapping: CallArgMapping,
        existing_subst: &'arg Substitution,
    ) -> Self {
        Self {
            generics,
            params,
            arg_tys,
            arg_mapping,
            existing_subst,
        }
    }

    pub fn infer(&self) -> Substitution {
        let mut subst = Substitution::new();
        for (param_ty, arg_ty) in self
            .params
            .iter()
            .skip(self.arg_mapping.first_param_idx())
            .zip(self.arg_tys)
        {
            self.infer_ty_subst(param_ty, arg_ty, &mut subst);
        }
        subst
    }

    fn infer_ty_subst(&self, param_ty: &Ty, arg_ty: &Ty, subst: &mut Substitution) {
        if let Ty::Param(param) = param_ty
            && self
                .generics
                .iter_self()
                .any(|candidate| candidate.param() == GenericParamRef::Type(*param))
        {
            self.push_inferred_call_subst(subst, *param, arg_ty);
            return;
        }

        match (param_ty, arg_ty) {
            (
                Ty::Reference {
                    mutability: param_mutability,
                    inner: param_inner,
                    ..
                },
                Ty::Reference {
                    mutability: arg_mutability,
                    inner: arg_inner,
                    ..
                },
            )
            | (
                Ty::RawPointer {
                    mutability: param_mutability,
                    inner: param_inner,
                },
                Ty::RawPointer {
                    mutability: arg_mutability,
                    inner: arg_inner,
                },
            ) if param_mutability == arg_mutability => {
                self.infer_ty_subst(param_inner, arg_inner, subst);
            }
            (Ty::Tuple(param_fields), Ty::Tuple(arg_fields))
                if param_fields.len() == arg_fields.len() =>
            {
                for (param_field, arg_field) in param_fields.iter().zip(arg_fields) {
                    self.infer_ty_subst(param_field, arg_field, subst);
                }
            }
            (Ty::Slice(param_inner), Ty::Slice(arg_inner))
            | (
                Ty::Array {
                    inner: param_inner, ..
                },
                Ty::Array {
                    inner: arg_inner, ..
                },
            ) => self.infer_ty_subst(param_inner, arg_inner, subst),
            (Ty::Adt(param), Ty::Adt(arg)) if param.def == arg.def => {
                for (param, arg) in param.args.iter().zip(&arg.args) {
                    if let (GenericArg::Type(param), GenericArg::Type(arg)) = (param, arg) {
                        self.infer_ty_subst(param, arg, subst);
                    }
                }
            }
            _ => {}
        }
    }

    fn push_inferred_call_subst(
        &self,
        subst: &mut Substitution,
        param: rg_ir_model::TypeParamRef,
        arg_ty: &Ty,
    ) {
        if matches!(arg_ty, Ty::Unknown) {
            return;
        }
        let key = GenericParamRef::Type(param);
        if self
            .existing_subst
            .get(key)
            .and_then(GenericArg::as_ty)
            .is_some_and(|ty| !matches!(ty, Ty::Unknown))
        {
            return;
        }
        if let Some(existing_ty) = subst.get(key).and_then(GenericArg::as_ty) {
            if existing_ty != arg_ty {
                subst.push(key, GenericArg::Type(Box::new(Ty::Unknown)));
            }
            return;
        }
        subst.push(key, GenericArg::Type(Box::new(arg_ty.clone())));
    }
}

/// Seed a function's own type parameters as unknown without shadowing parent IDs.
pub fn function_generic_shadow_subst(generics: &Generics<'_>) -> Substitution {
    let mut subst = Substitution::new();
    for param in generics.iter_self() {
        if let GenericParamRef::Type(param) = param.param() {
            subst.push(
                GenericParamRef::Type(param),
                GenericArg::Type(Box::new(Ty::Unknown)),
            );
        }
    }
    subst
}
