//! Apply receiver and written generic arguments to a selected call signature.
//!
//! This projection uses the types available to the query. Live call inference retains and refines
//! its own variables after a target has been selected.

use rg_def_map::DefMapSource;
use rg_ir_model::{ExprId, GenericDefRef, GenericParamRef};
use rg_item_tree::FunctionQualifiers;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{GenericParamSource, Generics, ItemStoreSource};
use rg_ty::lowering::CallableSignature;
use rg_ty::{GenericArg, Substitution, Ty};

use super::{BodyCallQuery, ResolvedCallTarget};

/// Projects a selected call target into parameter and return types.
pub(crate) struct CallSignature<'call, 'query, D, I> {
    pub(crate) query: &'call BodyCallQuery<'query, D, I>,
    pub(crate) target: &'call ResolvedCallTarget,
}

/// Signature facts projected for one selected call target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CallProjection {
    signature: CallableSignature,
    subst: Substitution,
}

impl CallProjection {
    /// Return the uninstantiated semantic signature selected for this call.
    pub(crate) fn signature(&self) -> &CallableSignature {
        &self.signature
    }

    /// Return the call-specific substitution used to project signature types.
    pub(crate) fn subst(&self) -> &Substitution {
        &self.subst
    }
}

impl<'call, 'query, D, I> CallSignature<'call, 'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    /// Project written parameter types and result type for this selected call.
    pub(crate) fn project(&self, args: &[ExprId]) -> Result<CallProjection, PackageStoreError> {
        let Some(signature) = self
            .query
            .context
            .signatures()
            .function(self.target.function())?
        else {
            return Ok(CallProjection {
                signature: CallableSignature {
                    params: Vec::new(),
                    ret: Ty::Unknown,
                    clauses: Vec::new(),
                    qualifiers: FunctionQualifiers::default(),
                },
                subst: Substitution::new(),
            });
        };
        let generics = self
            .query
            .context
            .item_paths()
            .generics()
            .generics(GenericDefRef::Function(self.target.function()))?;
        let base_subst = self.base_subst(&generics)?;

        let mut return_subst = base_subst.clone();
        let arg_tys = args
            .iter()
            .map(|arg| {
                self.query
                    .context
                    .query_body()
                    .expr_ty_unchecked(*arg)
                    .clone()
            })
            .collect::<Vec<_>>();
        let inferred_arg_subst =
            self.infer_argument_subst(&generics, &signature.params, &arg_tys, &return_subst);
        return_subst.extend(inferred_arg_subst);

        Ok(CallProjection {
            signature,
            subst: return_subst,
        })
    }

    /// Derive function-owned bindings available from matching argument and parameter shapes.
    ///
    /// This projection operates on stable `Ty`, not inference variables, so conflicting evidence
    /// becomes `Unknown`. Inherited trait and impl parameters retain the bindings selected from
    /// the call receiver.
    fn infer_argument_subst(
        &self,
        generics: &Generics<'_>,
        params: &[Ty],
        arg_tys: &[Ty],
        existing_subst: &Substitution,
    ) -> Substitution {
        let mut subst = Substitution::new();
        for (param_ty, arg_ty) in params
            .iter()
            .skip(self.target.first_written_param_idx())
            .zip(arg_tys)
        {
            Self::infer_ty_subst(generics, existing_subst, param_ty, arg_ty, &mut subst);
        }
        subst
    }

    /// Follow compatible type structure until a function-owned parameter is reached.
    fn infer_ty_subst(
        generics: &Generics<'_>,
        existing_subst: &Substitution,
        param_ty: &Ty,
        arg_ty: &Ty,
        subst: &mut Substitution,
    ) {
        if let Ty::Param(param) = param_ty
            && generics
                .iter_self()
                .any(|candidate| candidate.param() == GenericParamRef::Type(*param))
        {
            if matches!(arg_ty, Ty::Unknown) {
                return;
            }

            let key = GenericParamRef::Type(*param);
            if existing_subst
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
                Self::infer_ty_subst(generics, existing_subst, param_inner, arg_inner, subst);
            }
            (Ty::Tuple(param_fields), Ty::Tuple(arg_fields))
                if param_fields.len() == arg_fields.len() =>
            {
                for (param_field, arg_field) in param_fields.iter().zip(arg_fields) {
                    Self::infer_ty_subst(generics, existing_subst, param_field, arg_field, subst);
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
            ) => Self::infer_ty_subst(generics, existing_subst, param_inner, arg_inner, subst),
            (Ty::Adt(param), Ty::Adt(arg)) if param.def == arg.def => {
                for (param, arg) in param.args.iter().zip(&arg.args) {
                    if let (GenericArg::Type(param), GenericArg::Type(arg)) = (param, arg) {
                        Self::infer_ty_subst(generics, existing_subst, param, arg, subst);
                    }
                }
            }
            _ => {}
        }
    }

    /// Combine receiver, unresolved function, and explicit generic substitutions.
    fn base_subst(
        &self,
        generics: &rg_semantic_ir::Generics<'_>,
    ) -> Result<Substitution, PackageStoreError> {
        let mut subst = self.target.self_source.base_subst();

        // A trait's synthetic `Self` parameter is part of the parent's canonical generic list.
        // Method lookup already selected its concrete receiver, so bind that identity here rather
        // than introducing a spelling-based `"Self"` substitution.
        if let Some(self_ty) = self.target.self_source.self_ty()
            && let Some(self_param) = generics.iter().find_map(|param| {
                matches!(param.source(), GenericParamSource::TraitSelf).then_some(param.param())
            })
        {
            subst.push(self_param, GenericArg::Type(Box::new(self_ty)));
        }

        // Receiver substitutions describe parent trait or impl parameters. A function's own type
        // parameters begin unresolved, then explicit arguments or argument shapes replace these
        // entries. Iterating only this owner keeps inherited bindings intact.
        for param in generics.iter_self() {
            if let GenericParamRef::Type(param) = param.param() {
                subst.push(
                    GenericParamRef::Type(param),
                    GenericArg::Type(Box::new(Ty::Unknown)),
                );
            }
        }
        subst.extend(self.explicit_subst()?);
        Ok(subst)
    }

    /// Bind written function generics at the call-site scope.
    fn explicit_subst(&self) -> Result<Substitution, PackageStoreError> {
        if self.target.explicit_args().is_empty() {
            return Ok(Substitution::new());
        }

        // Function turbofish arguments are supplied at the call site, so names inside them must
        // resolve from the body scope where the call was written.
        self.query.context.generics().subst_for_explicit_args(
            GenericDefRef::Function(self.target.function()),
            self.target.explicit_args(),
            self.target.site_scope(),
        )
    }
}
