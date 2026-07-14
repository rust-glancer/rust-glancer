//! Instantiation of owner-scoped semantic generic parameters.
//!
//! Source names are resolved before this module is involved. Substitutions therefore use
//! [`GenericParamRef`] keys, which keeps an impl's `T` distinct from an associated method's `T`
//! even when both are visible in the same signature.

use rg_ir_model::{GenericParamRef, TypeParamRef};
use rg_semantic_ir::Generics;

use crate::{
    AdtTy, AliasTy, AssocTypeBinding, Clause, ConstValue, FnDefTy, GenericArg, GenericArgs,
    Lifetime, OpaqueTy, ProjectionTy, TraitApplication, TraitRefLowering, Ty,
};

/// Semantic substitution keyed by owner-scoped parameter identity.
///
/// The order mirrors `Generics`, but lookup still checks the ID so parameters with the same source
/// name in a parent and child owner cannot replace one another accidentally.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Substitution(Vec<(GenericParamRef, GenericArg)>);

impl Substitution {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_args(generics: &Generics<'_>, args: &GenericArgs) -> Self {
        Self(
            generics
                .iter()
                .zip(args.iter().cloned())
                .map(|(param, arg)| (param.param(), arg))
                .collect(),
        )
    }

    pub fn identity(generics: &Generics<'_>) -> Self {
        Self(
            generics
                .iter()
                .map(|param| {
                    let param = param.param();
                    let arg = match param {
                        GenericParamRef::Lifetime(param) => {
                            GenericArg::Lifetime(Lifetime::Param(param))
                        }
                        GenericParamRef::Type(param) => {
                            GenericArg::Type(Box::new(Ty::Param(param)))
                        }
                        GenericParamRef::Const(param) => {
                            GenericArg::Const(ConstValue::Param(param))
                        }
                    };
                    (param, arg)
                })
                .collect(),
        )
    }

    pub fn push(&mut self, param: GenericParamRef, arg: GenericArg) {
        if let Some((_, value)) = self.0.iter_mut().find(|(candidate, _)| *candidate == param) {
            *value = arg;
        } else {
            self.0.push((param, arg));
        }
    }

    pub fn extend(&mut self, other: Self) {
        for (param, arg) in other.0 {
            self.push(param, arg);
        }
    }

    pub fn get(&self, param: GenericParamRef) -> Option<&GenericArg> {
        self.0
            .iter()
            .rev()
            .find_map(|(candidate, arg)| (*candidate == param).then_some(arg))
    }

    pub fn type_param(&self, param: TypeParamRef) -> Option<&Ty> {
        self.get(GenericParamRef::Type(param))?.as_ty()
    }

    pub fn args_for(&self, generics: &Generics<'_>) -> GenericArgs {
        generics
            .iter()
            .map(|param| {
                self.get(param.param())
                    .cloned()
                    .unwrap_or_else(|| Self::unknown_arg(param.param()))
            })
            .collect()
    }

    /// Apply this substitution to every semantic parameter position in a type.
    pub fn apply(&self, ty: &Ty) -> Ty {
        match ty {
            Ty::Param(param) => self
                .type_param(*param)
                .cloned()
                .unwrap_or(Ty::Param(*param)),
            Ty::Tuple(fields) => Ty::tuple(fields.iter().map(|field| self.apply(field)).collect()),
            Ty::Array { inner, len } => Ty::Array {
                inner: Box::new(self.apply(inner)),
                len: self.apply_const(*len),
            },
            Ty::Slice(inner) => Ty::Slice(Box::new(self.apply(inner))),
            Ty::Reference {
                lifetime,
                mutability,
                inner,
            } => Ty::Reference {
                lifetime: self.apply_lifetime(*lifetime),
                mutability: *mutability,
                inner: Box::new(self.apply(inner)),
            },
            Ty::RawPointer { mutability, inner } => Ty::RawPointer {
                mutability: *mutability,
                inner: Box::new(self.apply(inner)),
            },
            Ty::FnPointer { params, ret } => Ty::FnPointer {
                params: params.iter().map(|param| self.apply(param)).collect(),
                ret: Box::new(self.apply(ret)),
            },
            Ty::Adt(ty) => Ty::Adt(AdtTy {
                def: ty.def,
                args: ty.args.iter().map(|arg| self.apply_arg(arg)).collect(),
            }),
            Ty::Alias(alias) => Ty::Alias(match alias {
                AliasTy::Projection(alias) => AliasTy::Projection(ProjectionTy {
                    associated_ty: alias.associated_ty,
                    args: alias.args.iter().map(|arg| self.apply_arg(arg)).collect(),
                }),
                AliasTy::Opaque(alias) => AliasTy::Opaque(OpaqueTy {
                    opaque: alias.opaque,
                    args: alias.args.iter().map(|arg| self.apply_arg(arg)).collect(),
                }),
            }),
            Ty::FnDef(function) => Ty::FnDef(FnDefTy {
                def: function.def,
                args: function
                    .args
                    .iter()
                    .map(|arg| self.apply_arg(arg))
                    .collect(),
            }),
            Ty::Unit
            | Ty::Never
            | Ty::Primitive(_)
            | Ty::Closure(_)
            | Ty::Unknown
            | Ty::InferVar { .. } => ty.clone(),
        }
    }

    pub fn apply_arg(&self, arg: &GenericArg) -> GenericArg {
        match arg {
            GenericArg::Type(ty) => GenericArg::Type(Box::new(self.apply(ty))),
            GenericArg::Lifetime(lifetime) => GenericArg::Lifetime(self.apply_lifetime(*lifetime)),
            GenericArg::Const(ConstValue::Param(param)) => self
                .get(GenericParamRef::Const(*param))
                .cloned()
                .unwrap_or_else(|| arg.clone()),
            GenericArg::Const(_) => arg.clone(),
        }
    }

    pub fn apply_args(&self, args: &GenericArgs) -> GenericArgs {
        args.iter().map(|arg| self.apply_arg(arg)).collect()
    }

    pub fn apply_trait_application(&self, application: &TraitApplication) -> TraitApplication {
        TraitApplication {
            def: application.def,
            args: self.apply_args(&application.args),
        }
    }

    pub fn apply_trait_ref(&self, trait_ref: &TraitRefLowering) -> TraitRefLowering {
        TraitRefLowering {
            application: self.apply_trait_application(&trait_ref.application),
            associated_types: trait_ref
                .associated_types
                .iter()
                .map(|binding| AssocTypeBinding {
                    associated_ty: binding.associated_ty,
                    ty: self.apply(&binding.ty),
                })
                .collect(),
        }
    }

    pub fn apply_clause(&self, clause: &Clause) -> Clause {
        match clause {
            Clause::Implemented(application) => {
                Clause::Implemented(self.apply_trait_application(application))
            }
            Clause::AliasEq { alias, ty } => Clause::AliasEq {
                alias: ProjectionTy {
                    associated_ty: alias.associated_ty,
                    args: self.apply_args(&alias.args),
                },
                ty: self.apply(ty),
            },
        }
    }

    fn apply_const(&self, value: ConstValue) -> ConstValue {
        let ConstValue::Param(param) = value else {
            return value;
        };
        match self.get(GenericParamRef::Const(param)) {
            Some(GenericArg::Const(value)) => *value,
            Some(GenericArg::Type(_)) | Some(GenericArg::Lifetime(_)) | None => {
                ConstValue::Param(param)
            }
        }
    }

    fn apply_lifetime(&self, lifetime: Lifetime) -> Lifetime {
        let Lifetime::Param(param) = lifetime else {
            return lifetime;
        };
        match self.get(GenericParamRef::Lifetime(param)) {
            Some(GenericArg::Lifetime(lifetime)) => *lifetime,
            Some(GenericArg::Type(_)) | Some(GenericArg::Const(_)) | None => Lifetime::Param(param),
        }
    }

    pub(crate) fn unknown_arg(param: GenericParamRef) -> GenericArg {
        match param {
            GenericParamRef::Lifetime(_) => GenericArg::Lifetime(Lifetime::Erased),
            GenericParamRef::Type(_) => GenericArg::Type(Box::new(Ty::Unknown)),
            GenericParamRef::Const(_) => GenericArg::Const(ConstValue::Unknown),
        }
    }
}

#[cfg(test)]
mod tests {
    use rg_ir_model::{
        CrateId, CrateRef, DefMapRef, FunctionId, FunctionRef, GenericDefRef, ImplId, ImplRef,
        LocalTypeOrConstParamId, PackageSlot, TypeParamRef, items::PrimitiveTy,
    };

    use super::Substitution;
    use crate::{GenericArg, Ty};

    #[test]
    fn same_local_id_from_parent_and_child_keeps_distinct_bindings() {
        let origin = DefMapRef::Crate(CrateRef {
            package: PackageSlot(0),
            crate_id: CrateId(0),
        });
        let parent = TypeParamRef {
            owner: GenericDefRef::Impl(ImplRef {
                origin,
                id: ImplId(0),
            }),
            local_id: LocalTypeOrConstParamId(0),
        };
        let child = TypeParamRef {
            owner: GenericDefRef::Function(FunctionRef {
                origin,
                id: FunctionId(0),
            }),
            local_id: LocalTypeOrConstParamId(0),
        };
        let mut subst = Substitution::new();
        subst.push(
            parent.into(),
            GenericArg::Type(Box::new(Ty::Primitive(PrimitiveTy::Bool))),
        );
        subst.push(
            child.into(),
            GenericArg::Type(Box::new(Ty::Primitive(PrimitiveTy::Char))),
        );

        assert_eq!(
            subst.apply(&Ty::Param(parent)),
            Ty::Primitive(PrimitiveTy::Bool)
        );
        assert_eq!(
            subst.apply(&Ty::Param(child)),
            Ty::Primitive(PrimitiveTy::Char)
        );
    }
}
