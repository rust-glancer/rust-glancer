//! Callable return type recovery for body expression resolution.
//!
//! Calls and method calls both need to combine declared signatures with receiver substitutions,
//! explicit generic arguments, and direct argument inference. Keeping that machinery here lets
//! expression traversal stay focused on expression shapes.

use rg_ir_model::{
    DefId, ExprData, ExprId, FunctionRef, ImplRef, ItemOwner, ScopeId, SemanticItemRef,
    identity::DeclarationRef,
    items::{GenericArg as ItemGenericArg, GenericParams, TypeRef},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_std::UniqueVec;
use rg_ty::{
    CallArgInference, CallArgMapping, NominalTy, Ty, TypeSubst, function_generic_shadow_subst,
};

use crate::resolution::{BodyResolutionContext, TypeRefUseSite};
use crate::{ir::ExprKind, ir::resolved::BodyResolution};

pub(crate) struct CallableReturnQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> CallableReturnQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    pub(crate) fn call_expr_ty(
        &self,
        callee: Option<ExprId>,
        args: &[ExprId],
    ) -> Result<Ty, PackageStoreError> {
        let Some(callee) = callee else {
            return Ok(Ty::Unknown);
        };
        let callee_data = self.context.body().expr_unchecked(callee);
        let callee_ty = self.context.body().expr_ty_unchecked(callee);

        if matches!(callee_ty, Ty::Nominal(_) | Ty::SelfTy(_)) {
            return Ok(callee_ty.clone());
        }

        // Ordinary calls use declared return types plus a deliberately-small substitution model:
        // explicit turbofish args and direct argument-to-parameter type inference.
        let mut return_tys = UniqueVec::new();
        match self.context.body().expr_resolution(callee) {
            BodyResolution::Declarations(declarations) => {
                for declaration in declarations {
                    self.push_return_ty_for_declaration(
                        *declaration,
                        &mut return_tys,
                        Self::explicit_callee_generic_args(callee_data),
                        callee_data.scope,
                        args,
                    )?;
                }
            }
            BodyResolution::Binding(_) | BodyResolution::Unknown => {}
        }

        Ok(match return_tys.as_slice() {
            [ty] => ty.clone(),
            [] | [_, ..] => Ty::Unknown,
        })
    }

    pub(crate) fn return_ty_with_call_args(
        &self,
        function_ref: FunctionRef,
        receiver_ty: Option<&NominalTy>,
        explicit_args: &[ItemGenericArg],
        args: &[ExprId],
        arg_mapping: CallArgMapping,
        call_scope: Option<ScopeId>,
    ) -> Result<Ty, PackageStoreError> {
        let Some(function_data) = self.context.item_query().function_data(function_ref)? else {
            return Ok(Ty::Unknown);
        };
        let subst = receiver_ty
            .map(|ty| {
                // Receiver type args and impl self args both contribute substitutions. For
                // `impl<U> Wrapper<U>`, this maps `U` to the known receiver argument.
                let mut subst = self.semantic_type_subst(ty)?;
                subst.extend(self.impl_self_subst_for_function(
                    function_ref,
                    function_data.owner,
                    ty,
                )?);
                Ok(subst)
            })
            .transpose()?
            .unwrap_or_default();
        self.return_ty_with_subst_and_call_args(
            function_ref,
            receiver_ty
                .cloned()
                .map(|ty| Ty::nominal([ty].into_iter().collect())),
            subst,
            explicit_args,
            args,
            arg_mapping,
            call_scope,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn return_ty_with_subst_and_call_args(
        &self,
        function_ref: FunctionRef,
        self_ty: Option<Ty>,
        mut subst: TypeSubst,
        explicit_args: &[ItemGenericArg],
        args: &[ExprId],
        arg_mapping: CallArgMapping,
        call_scope: Option<ScopeId>,
    ) -> Result<Ty, PackageStoreError> {
        let item_query = self.context.item_query();
        let Some(function_data) = item_query.function_data(function_ref)? else {
            return Ok(Ty::Unknown);
        };
        subst.extend(function_generic_shadow_subst(
            function_data.signature.generics(),
        ));
        if let Some(call_scope) = call_scope {
            subst.extend(self.explicit_function_subst(
                function_data.signature.generics(),
                explicit_args,
                call_scope,
            )?);
        }
        let arg_tys = args
            .iter()
            .map(|arg| self.context.body().expr_ty_unchecked(*arg).clone())
            .collect::<Vec<_>>();
        let inferred_subst = CallArgInference::new(
            function_data.signature.generics(),
            function_data.signature.params(),
            &arg_tys,
            arg_mapping,
            &subst,
        )
        .infer();
        subst.extend(inferred_subst);
        self.return_ty_with_resolved_subst(function_ref, self_ty, subst)
    }

    /// Returns the explicitly declared return type for a function body, if one was written.
    ///
    /// This is the expected type for `return expr` and the body tail. Functions without `-> T`
    /// are left to ordinary expression typing so this pass does not erase useful invalid-code
    /// facts by forcing an implicit `()`.
    pub(crate) fn explicit_declared_return_ty(
        &self,
        function_ref: FunctionRef,
    ) -> Result<Option<Ty>, PackageStoreError> {
        let item_query = self.context.item_query();
        let Some(function_data) = item_query.function_data(function_ref)? else {
            return Ok(None);
        };
        let Some(ret_ty) = function_data.signature.ret_ty() else {
            return Ok(None);
        };
        let subst = function_generic_shadow_subst(function_data.signature.generics());

        self.resolve_declared_return_ty(function_ref, None, &subst, ret_ty)
            .map(Some)
    }

    fn return_ty_with_resolved_subst(
        &self,
        function_ref: FunctionRef,
        self_ty: Option<Ty>,
        subst: TypeSubst,
    ) -> Result<Ty, PackageStoreError> {
        let item_query = self.context.item_query();
        let Some(function_data) = item_query.function_data(function_ref)? else {
            return Ok(Ty::Unknown);
        };
        let Some(ret_ty) = function_data.signature.ret_ty() else {
            return Ok(Ty::Unit);
        };

        self.resolve_declared_return_ty(function_ref, self_ty, &subst, ret_ty)
    }

    fn resolve_declared_return_ty(
        &self,
        function_ref: FunctionRef,
        self_ty: Option<Ty>,
        subst: &TypeSubst,
        ret_ty: &TypeRef,
    ) -> Result<Ty, PackageStoreError> {
        if ret_ty.is_self_type() {
            return Ok(match self_ty {
                Some(self_ty) => self_ty,
                None => Ty::self_ty(
                    self.context
                        .type_path_query()
                        .self_nominal_tys_for_function(function_ref)?,
                ),
            });
        }

        self.context
            .type_path_query()
            .type_ref(TypeRefUseSite::Function(function_ref))
            .with_subst(subst)
            .resolve(ret_ty)
    }

    fn push_return_ty_for_declaration(
        &self,
        declaration: DeclarationRef,
        return_tys: &mut UniqueVec<Ty>,
        explicit_args: &[ItemGenericArg],
        call_scope: ScopeId,
        args: &[ExprId],
    ) -> Result<(), PackageStoreError> {
        match declaration {
            DeclarationRef::LocalDef(local_def) => {
                let Some(function_ref) = self.function_ref_for_def(DefId::Local(local_def))? else {
                    return Ok(());
                };
                return_tys.push(self.return_ty_with_call_args(
                    function_ref,
                    None,
                    explicit_args,
                    args,
                    CallArgMapping::FunctionCall,
                    Some(call_scope),
                )?);
            }
            DeclarationRef::Item(SemanticItemRef::Function(function_ref)) => {
                return_tys.push(self.return_ty_with_call_args(
                    function_ref,
                    None,
                    explicit_args,
                    args,
                    CallArgMapping::FunctionCall,
                    Some(call_scope),
                )?);
            }
            DeclarationRef::Module(_)
            | DeclarationRef::Item(
                SemanticItemRef::TypeDef(_)
                | SemanticItemRef::Trait(_)
                | SemanticItemRef::Impl(_)
                | SemanticItemRef::TypeAlias(_)
                | SemanticItemRef::Const(_)
                | SemanticItemRef::Static(_),
            )
            | DeclarationRef::Field(_)
            | DeclarationRef::EnumVariant(_)
            | DeclarationRef::BodyBinding(_) => {}
        }

        Ok(())
    }

    fn explicit_callee_generic_args(callee_data: &ExprData) -> &[ItemGenericArg] {
        // A normal call expression has a callee expression, so `make::<T>()` and
        // `Type::build::<T>()` carry call generics on the final callee path segment. Method calls
        // are a different ExprKind and store their method-name generics directly.
        match &callee_data.kind {
            ExprKind::Path { path } => path.last_segment_angle_args().unwrap_or(&[]),
            _ => &[],
        }
    }

    fn explicit_function_subst(
        &self,
        generics: Option<&GenericParams>,
        explicit_args: &[ItemGenericArg],
        scope: ScopeId,
    ) -> Result<TypeSubst, PackageStoreError> {
        let Some(generics) = generics else {
            return Ok(TypeSubst::new());
        };
        if explicit_args.is_empty() {
            return Ok(TypeSubst::new());
        }

        // Function turbofish arguments are supplied at the call site, so names inside them must
        // resolve from the body scope where the call was written.
        let type_paths = self.context.type_path_query();
        let arg_resolver = type_paths.type_ref(TypeRefUseSite::Scope(scope));
        let generic_args = explicit_args
            .iter()
            .map(|arg| arg_resolver.generic_arg(arg))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TypeSubst::from_generics(generics, &generic_args))
    }

    fn impl_self_subst_for_function(
        &self,
        function_ref: FunctionRef,
        owner: ItemOwner,
        receiver_ty: &NominalTy,
    ) -> Result<TypeSubst, PackageStoreError> {
        let ItemOwner::Impl(impl_id) = owner else {
            return Ok(TypeSubst::new());
        };
        let item_query = self.context.item_query();
        let Some(impl_data) = item_query.impl_data(ImplRef {
            origin: function_ref.origin,
            id: impl_id,
        })?
        else {
            return Ok(TypeSubst::new());
        };

        Ok(self
            .context
            .impl_matcher()
            .impl_self_subst_for_impl(impl_data, receiver_ty))
    }

    fn semantic_type_subst(&self, ty: &NominalTy) -> Result<TypeSubst, PackageStoreError> {
        Ok(self
            .context
            .item_query()
            .generic_params_for_type_def(ty.def)?
            .map(|generics| TypeSubst::from_generics(generics, &ty.args))
            .unwrap_or_else(TypeSubst::new))
    }

    fn function_ref_for_def(&self, def: DefId) -> Result<Option<FunctionRef>, PackageStoreError> {
        let DefId::Local(local_def) = def else {
            return Ok(None);
        };
        Ok(
            match self
                .context
                .item_query()
                .semantic_item_for_local_def(local_def)?
            {
                Some(SemanticItemRef::Function(function)) => Some(function),
                Some(_) | None => None,
            },
        )
    }
}
