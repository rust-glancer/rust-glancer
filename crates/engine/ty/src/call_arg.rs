use rg_ir_model::{
    Mutability,
    items::{GenericParams, ParamItem, TypeRef},
};
use rg_text::Name;

use crate::{Ty, TypeSubst};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallArgMapping {
    /// The written call args line up with signature params from the beginning.
    FunctionCall,
    /// Method-call syntax stores the receiver separately, so written args start after `self`.
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

/// Infers direct function-generic substitutions from already-known call argument types.
pub struct CallArgInference<'signature, 'arg> {
    generics: Option<&'signature GenericParams>,
    params: &'signature [ParamItem],
    arg_tys: &'arg [Ty],
    arg_mapping: CallArgMapping,
    existing_subst: &'arg TypeSubst,
}

impl<'signature, 'arg> CallArgInference<'signature, 'arg> {
    pub fn new(
        generics: Option<&'signature GenericParams>,
        params: &'signature [ParamItem],
        arg_tys: &'arg [Ty],
        arg_mapping: CallArgMapping,
        existing_subst: &'arg TypeSubst,
    ) -> Self {
        Self {
            generics,
            params,
            arg_tys,
            arg_mapping,
            existing_subst,
        }
    }

    pub fn infer(&self) -> TypeSubst {
        let Some(generics) = self.generics else {
            return TypeSubst::new();
        };
        if self.arg_tys.is_empty() {
            return TypeSubst::new();
        }

        let type_params = generics
            .types
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>();
        let mut subst = TypeSubst::new();

        for (param, arg_ty) in self
            .params
            .iter()
            .skip(self.arg_mapping.first_param_idx())
            .zip(self.arg_tys)
        {
            let Some(param_ty) = &param.ty else {
                continue;
            };
            self.infer_type_ref_subst(param_ty, arg_ty, &type_params, &mut subst);
        }

        subst
    }

    fn infer_type_ref_subst(
        &self,
        param_ty: &TypeRef,
        arg_ty: &Ty,
        type_params: &[&str],
        subst: &mut TypeSubst,
    ) {
        // This is intentionally a small, syntax-directed matcher. It binds `T` from positions
        // where the written parameter type directly exposes `T`; nominal containers like
        // `Vec<T>` are left for a later, more deliberate unification step.
        if let Some(name) = param_ty.type_param_name()
            && type_params.contains(&name.as_str())
        {
            self.push_inferred_call_subst(subst, name, arg_ty);
            return;
        }

        match (param_ty, arg_ty) {
            (
                TypeRef::Reference {
                    mutability: param_mutability,
                    inner: param_inner,
                    ..
                },
                Ty::Reference {
                    mutability: arg_mutability,
                    inner: arg_inner,
                },
            ) if Self::ref_mutability_matches(*param_mutability, *arg_mutability) => {
                self.infer_type_ref_subst(param_inner, arg_inner, type_params, subst);
            }
            (TypeRef::Tuple(param_fields), Ty::Tuple(arg_fields))
                if param_fields.len() == arg_fields.len() =>
            {
                for (param_field, arg_field) in param_fields.iter().zip(arg_fields) {
                    self.infer_type_ref_subst(param_field, arg_field, type_params, subst);
                }
            }
            (TypeRef::Slice(param_inner), Ty::Slice(arg_inner)) => {
                self.infer_type_ref_subst(param_inner, arg_inner, type_params, subst);
            }
            (
                TypeRef::Array {
                    inner: param_inner,
                    len: param_len,
                },
                Ty::Array {
                    inner: arg_inner,
                    len: arg_len,
                },
            ) if param_len == arg_len => {
                self.infer_type_ref_subst(param_inner, arg_inner, type_params, subst);
            }
            _ => {}
        }
    }

    fn push_inferred_call_subst(&self, subst: &mut TypeSubst, name: Name, arg_ty: &Ty) {
        if !Self::ty_is_inferable_arg(arg_ty) {
            return;
        }

        if let Some(existing_ty) = self.existing_subst.get(name.as_str())
            && !matches!(existing_ty, Ty::Unknown)
        {
            return;
        }

        if let Some(existing_ty) = subst.get(name.as_str()) {
            if matches!(existing_ty, Ty::Unknown) || existing_ty == arg_ty {
                return;
            }

            subst.push(name, Ty::Unknown);
            return;
        }

        subst.push(name, arg_ty.clone());
    }

    fn ref_mutability_matches(param_mutability: Mutability, arg_mutability: Mutability) -> bool {
        param_mutability == arg_mutability
    }

    fn ty_is_inferable_arg(ty: &Ty) -> bool {
        !matches!(ty, Ty::Unknown | Ty::Syntax(_))
    }
}

/// Function generics shadow outer impl generics with the same names.
pub fn function_generic_shadow_subst(generics: Option<&GenericParams>) -> TypeSubst {
    let Some(generics) = generics else {
        return TypeSubst::new();
    };

    // Seed function generics as unknown first; explicit turbofish args and argument inference can
    // overwrite these placeholders once call-site information is available.
    generics
        .types
        .iter()
        .map(|param| (param.name.clone(), Ty::Unknown))
        .collect()
}
