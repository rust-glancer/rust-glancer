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
    AutoderefMode, CallArgInference, CallArgMapping, NominalTy, Ty, TypeSubst,
    function_generic_shadow_subst,
};

use crate::resolution::{BodyResolutionContext, TypeRefUseSite};
use crate::{ir::ExprKind, ir::resolved::BodyResolution};

/// Function selected by call syntax, plus the call-site generic args written for that function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedCallable {
    function_ref: FunctionRef,
    explicit_args: Vec<ItemGenericArg>,
}

impl SelectedCallable {
    pub(crate) fn function_ref(&self) -> FunctionRef {
        self.function_ref
    }

    pub(crate) fn explicit_args(&self) -> &[ItemGenericArg] {
        &self.explicit_args
    }
}

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

    /// Returns declared parameter types for a uniquely resolved ordinary function call.
    ///
    /// This is signature evidence only: explicit turbofish args apply, but the arguments being
    /// constrained do not feed back into generic inference here.
    pub(crate) fn function_call_param_tys(
        &self,
        callee: ExprId,
    ) -> Result<Option<Vec<Ty>>, PackageStoreError> {
        let callee_data = self.context.body().expr_unchecked(callee);
        let BodyResolution::Declarations(declarations) =
            self.context.body().expr_resolution(callee)
        else {
            return Ok(None);
        };
        let [declaration] = declarations.as_slice() else {
            return Ok(None);
        };

        self.param_tys_for_declaration(
            *declaration,
            Self::explicit_callee_generic_args(callee_data),
            callee_data.scope,
            CallArgMapping::FunctionCall,
        )
    }

    /// Returns the function selected by a uniquely resolved ordinary call.
    pub(crate) fn selected_callable_for_call(
        &self,
        callee: Option<ExprId>,
    ) -> Result<Option<SelectedCallable>, PackageStoreError> {
        let Some(callee) = callee else {
            return Ok(None);
        };
        let callee_data = self.context.body().expr_unchecked(callee);

        self.selected_callable_for_resolution(
            self.context.body().expr_resolution(callee),
            Self::explicit_callee_generic_args(callee_data),
        )
    }

    /// Returns the function selected by a uniquely resolved method call.
    pub(crate) fn selected_callable_for_method_call(
        &self,
        method_call: ExprId,
        explicit_args: &[ItemGenericArg],
    ) -> Result<Option<SelectedCallable>, PackageStoreError> {
        self.selected_callable_for_resolution(
            self.context.body().expr_resolution(method_call),
            explicit_args,
        )
    }

    /// Returns declared parameter types for a uniquely resolved method call.
    ///
    /// This mirrors method lookup closely enough to apply receiver and impl substitutions before
    /// inference uses the signature as expected-type evidence for written call arguments.
    pub(crate) fn method_call_param_tys(
        &self,
        method_call: ExprId,
        receiver: ExprId,
        method_name: &str,
        explicit_args: &[ItemGenericArg],
    ) -> Result<Option<Vec<Ty>>, PackageStoreError> {
        let BodyResolution::Declarations(declarations) =
            self.context.body().expr_resolution(method_call)
        else {
            return Ok(None);
        };
        let [declaration] = declarations.as_slice() else {
            return Ok(None);
        };
        let Some(selected_function) = self.function_ref_for_declaration(*declaration)? else {
            return Ok(None);
        };

        let receiver_ty = self.context.body().expr_ty_unchecked(receiver);
        let call_scope = self.context.body().expr_unchecked(method_call).scope;
        let item_query = self.context.item_query();
        let mut current_depth = None;
        let mut param_tys = UniqueVec::new();

        for candidate in self
            .context
            .autoderef()
            .candidates(AutoderefMode::MethodReceiver, receiver_ty)
        {
            let candidate = candidate?;
            // Method lookup stops at the first autoderef depth that has matches. Keep expected
            // types tied to that same selected depth so later candidates cannot leak inward.
            if current_depth.is_some_and(|depth| depth != candidate.depth())
                && !param_tys.is_empty()
            {
                return Ok(Self::single_param_tys(&param_tys));
            }
            current_depth = Some(candidate.depth());

            for nominal_ty in candidate.ty().as_nominals() {
                for function_ref in self
                    .context
                    .receiver_functions()
                    .function_refs_for_receiver(nominal_ty, Some(method_name))?
                {
                    if function_ref != selected_function {
                        continue;
                    }
                    let Some(function_data) = item_query.function_data(function_ref)? else {
                        continue;
                    };
                    if function_data.name != method_name || !function_data.has_self_receiver() {
                        continue;
                    }

                    let mut subst = self.semantic_type_subst(nominal_ty)?;
                    subst.extend(self.impl_self_subst_for_function(
                        function_ref,
                        function_data.owner,
                        nominal_ty,
                    )?);
                    param_tys.push(self.param_tys_for_function_with_subst(
                        function_ref,
                        subst,
                        explicit_args,
                        call_scope,
                        CallArgMapping::MethodCall,
                    )?);
                }
            }

            // Structural receiver methods, such as slice methods, carry their receiver
            // substitution in the candidate because there is no nominal type key to reconstruct.
            for structural in self
                .context
                .receiver_functions()
                .structural_function_candidates_for_receiver(candidate.ty(), Some(method_name))?
            {
                let function_ref = structural.function();
                if function_ref != selected_function {
                    continue;
                }
                let Some(function_data) = item_query.function_data(function_ref)? else {
                    continue;
                };
                if function_data.name != method_name || !function_data.has_self_receiver() {
                    continue;
                }

                param_tys.push(self.param_tys_for_function_with_subst(
                    function_ref,
                    structural.subst().clone(),
                    explicit_args,
                    call_scope,
                    CallArgMapping::MethodCall,
                )?);
            }
        }

        Ok(Self::single_param_tys(&param_tys))
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
        let Some(function_ref) = self.function_ref_for_declaration(declaration)? else {
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

        Ok(())
    }

    fn param_tys_for_declaration(
        &self,
        declaration: DeclarationRef,
        explicit_args: &[ItemGenericArg],
        call_scope: ScopeId,
        arg_mapping: CallArgMapping,
    ) -> Result<Option<Vec<Ty>>, PackageStoreError> {
        let function_ref = self.function_ref_for_declaration(declaration)?;
        let Some(function_ref) = function_ref else {
            return Ok(None);
        };

        self.param_tys_for_function(function_ref, explicit_args, call_scope, arg_mapping)
            .map(Some)
    }

    fn param_tys_for_function(
        &self,
        function_ref: FunctionRef,
        explicit_args: &[ItemGenericArg],
        call_scope: ScopeId,
        arg_mapping: CallArgMapping,
    ) -> Result<Vec<Ty>, PackageStoreError> {
        self.param_tys_for_function_with_subst(
            function_ref,
            TypeSubst::new(),
            explicit_args,
            call_scope,
            arg_mapping,
        )
    }

    fn param_tys_for_function_with_subst(
        &self,
        function_ref: FunctionRef,
        mut subst: TypeSubst,
        explicit_args: &[ItemGenericArg],
        call_scope: ScopeId,
        arg_mapping: CallArgMapping,
    ) -> Result<Vec<Ty>, PackageStoreError> {
        let item_query = self.context.item_query();
        let Some(function_data) = item_query.function_data(function_ref)? else {
            return Ok(Vec::new());
        };
        subst.extend(function_generic_shadow_subst(
            function_data.signature.generics(),
        ));
        subst.extend(self.explicit_function_subst(
            function_data.signature.generics(),
            explicit_args,
            call_scope,
        )?);

        let first_param_idx = match arg_mapping {
            CallArgMapping::FunctionCall => 0,
            CallArgMapping::MethodCall => 1,
        };
        let type_paths = self.context.type_path_query();
        let param_resolver = type_paths
            .type_ref(TypeRefUseSite::Function(function_ref))
            .with_subst(&subst);

        // Keep one expected type per written argument. Missing parameter annotations are not
        // useful evidence, but `Unknown` preserves arity so the caller can still zip safely.
        function_data
            .signature
            .params()
            .iter()
            .skip(first_param_idx)
            .map(|param| {
                let Some(param_ty) = &param.ty else {
                    return Ok(Ty::Unknown);
                };

                param_resolver.resolve(param_ty)
            })
            .collect()
    }

    fn selected_callable_for_resolution(
        &self,
        resolution: &BodyResolution,
        explicit_args: &[ItemGenericArg],
    ) -> Result<Option<SelectedCallable>, PackageStoreError> {
        if let BodyResolution::Declarations(declarations) = resolution
            && let [declaration] = declarations.as_slice()
            && let Some(function_ref) = self.function_ref_for_declaration(*declaration)?
        {
            return Ok(Some(SelectedCallable {
                function_ref,
                explicit_args: explicit_args.to_vec(),
            }));
        }

        Ok(None)
    }

    fn single_param_tys(param_tys: &UniqueVec<Vec<Ty>>) -> Option<Vec<Ty>> {
        match param_tys.as_slice() {
            [param_tys] => Some(param_tys.clone()),
            [] | [_, ..] => None,
        }
    }

    fn function_ref_for_declaration(
        &self,
        declaration: DeclarationRef,
    ) -> Result<Option<FunctionRef>, PackageStoreError> {
        match declaration {
            DeclarationRef::LocalDef(local_def) => {
                self.function_ref_for_def(DefId::Local(local_def))
            }
            DeclarationRef::Item(SemanticItemRef::Function(function_ref)) => Ok(Some(function_ref)),
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
            | DeclarationRef::BodyBinding(_) => Ok(None),
        }
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
