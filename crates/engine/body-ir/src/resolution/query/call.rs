//! Function and method call resolution.

use rg_ir_model::{
    DefId, ExprData, ExprId, FunctionRef, ScopeId, SemanticItemRef,
    identity::DeclarationRef,
    items::{GenericArg as ItemGenericArg, GenericParams, TypeRef},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_std::{ExpectedUnique, UniqueVec};
use rg_ty::{
    CallArgInference, CallArgMapping, ExpectedNominalTyExt, ExpectedTyExt, Ty, TypeSubst,
    function_generic_shadow_subst,
};

use crate::resolution::{BodyResolutionContext, TypeRefUseSite};
use crate::{ir::ExprKind, ir::resolved::BodyResolution};

/// Function target selected by call syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedCallTarget {
    function: FunctionRef,
    explicit_args: Vec<ItemGenericArg>,
    site_scope: ScopeId,
    receiver: CallReceiver,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CallReceiver {
    None,
    Method(MethodReceiver),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MethodReceiver {
    self_ty: Ty,
    subst: TypeSubst,
}

impl ResolvedCallTarget {
    /// Build target data for an ordinary function call.
    fn function_call(
        function: FunctionRef,
        site_scope: ScopeId,
        explicit_args: &[ItemGenericArg],
    ) -> Self {
        Self {
            function,
            explicit_args: explicit_args.to_vec(),
            site_scope,
            receiver: CallReceiver::None,
        }
    }

    /// Build target data for a method call with receiver facts.
    fn method_call(
        function: FunctionRef,
        site_scope: ScopeId,
        explicit_args: &[ItemGenericArg],
        receiver: MethodReceiver,
    ) -> Self {
        Self {
            function,
            explicit_args: explicit_args.to_vec(),
            site_scope,
            receiver: CallReceiver::Method(receiver),
        }
    }

    /// Return the selected function.
    pub(crate) fn function(&self) -> FunctionRef {
        self.function
    }

    /// Return explicit generic arguments written at the call site.
    pub(crate) fn explicit_args(&self) -> &[ItemGenericArg] {
        &self.explicit_args
    }
}

impl CallReceiver {
    /// Choose how written arguments line up with declared params.
    fn arg_mapping(&self) -> CallArgMapping {
        match self {
            Self::None => CallArgMapping::FunctionCall,
            Self::Method(_) => CallArgMapping::MethodCall,
        }
    }

    /// Skip implicit receiver params when projecting written arguments.
    fn first_written_param_idx(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Method(_) => 1,
        }
    }

    /// Start signature projection with receiver-derived substitutions.
    fn base_subst(&self) -> TypeSubst {
        match self {
            Self::None => TypeSubst::new(),
            Self::Method(receiver) => receiver.subst.clone(),
        }
    }

    /// Return concrete receiver `Self` when this is a method call.
    fn self_ty(&self) -> Option<Ty> {
        match self {
            Self::None => None,
            Self::Method(receiver) => Some(receiver.self_ty.clone()),
        }
    }
}

/// A written function-call or method-call site.
pub(crate) enum CallSite<'a> {
    Function { callee: ExprId },
    Method(MethodCallSite<'a>),
}

/// Method-call syntax facts needed for method lookup.
pub(crate) struct MethodCallSite<'a> {
    pub(crate) receiver: ExprId,
    pub(crate) name: &'a str,
    pub(crate) explicit_args: &'a [ItemGenericArg],
    pub(crate) scope: ScopeId,
}

/// Call targets selected for one call expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedCallTargets {
    targets: UniqueVec<ResolvedCallTarget>,
}

impl ResolvedCallTargets {
    /// Start with no selected call targets.
    fn new() -> Self {
        Self {
            targets: UniqueVec::new(),
        }
    }

    /// Return whether call lookup found no targets.
    pub(crate) fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// Return function declarations for the selected call targets.
    pub(crate) fn resolution(&self) -> BodyResolution {
        let mut functions = UniqueVec::new();
        for target in &self.targets {
            functions.push(target.function());
        }

        if functions.is_empty() {
            BodyResolution::Unknown
        } else {
            BodyResolution::Declarations(functions.into_iter().map(DeclarationRef::from).collect())
        }
    }

    /// Return the unique projected return type, or unknown for zero or multiple targets.
    pub(crate) fn return_ty<'query, D, I>(
        &self,
        calls: &BodyCallQuery<'query, D, I>,
        args: &[ExprId],
    ) -> Result<Ty, PackageStoreError>
    where
        D: DefMapSource<Error = PackageStoreError> + Copy,
        I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
    {
        let mut return_tys = ExpectedUnique::new();
        for target in &self.targets {
            return_tys.push(calls.signature(target).return_ty(args)?);
        }

        Ok(return_tys.into_ty())
    }

    /// Add one target, preserving uniqueness.
    fn push(&mut self, target: ResolvedCallTarget) {
        self.targets.push(target);
    }

    /// Return the target only when lookup is unambiguous.
    fn single(&self) -> Option<ResolvedCallTarget> {
        match self.targets.as_slice() {
            [target] => Some(target.clone()),
            [] | [_, ..] => None,
        }
    }
}

/// Projects a selected call target into parameter and return types.
pub(crate) struct CallSignature<'call, 'query, D, I> {
    query: &'call BodyCallQuery<'query, D, I>,
    target: &'call ResolvedCallTarget,
}

/// Resolves function and method calls.
pub(crate) struct BodyCallQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyCallQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Return the result type of a call expression.
    pub(crate) fn call_expr_ty(
        &self,
        callee: Option<ExprId>,
        args: &[ExprId],
    ) -> Result<Ty, PackageStoreError> {
        let Some(callee) = callee else {
            return Ok(Ty::Unknown);
        };
        let callee_ty = self.context.body().expr_ty_unchecked(callee);

        if matches!(callee_ty, Ty::Nominal(_) | Ty::SelfTy(_)) {
            return Ok(callee_ty.clone());
        }

        // Ordinary calls use declared return types plus a deliberately-small substitution model:
        // explicit turbofish args and direct argument-to-parameter type inference.
        self.targets(CallSite::Function { callee })?
            .return_ty(self, args)
    }

    /// Return signature projection for a selected call target.
    pub(crate) fn signature<'call>(
        &'call self,
        target: &'call ResolvedCallTarget,
    ) -> CallSignature<'call, 'query, D, I> {
        CallSignature {
            query: self,
            target,
        }
    }

    /// Return the single target selected by a call expression.
    pub(crate) fn target(
        &self,
        call: ExprId,
    ) -> Result<Option<ResolvedCallTarget>, PackageStoreError> {
        let expr_data = self.context.body().expr_unchecked(call);
        let site = match &expr_data.kind {
            ExprKind::Call {
                callee: Some(callee),
                ..
            } => CallSite::Function { callee: *callee },
            ExprKind::Call { callee: None, .. } => return Ok(None),
            ExprKind::MethodCall {
                receiver: Some(receiver),
                method_name,
                generic_args,
                ..
            } => CallSite::Method(MethodCallSite {
                receiver: *receiver,
                name: method_name,
                explicit_args: generic_args,
                scope: expr_data.scope,
            }),
            ExprKind::MethodCall { receiver: None, .. } => return Ok(None),
            _ => return Ok(None),
        };

        Ok(self.targets(site)?.single())
    }

    /// Return all targets selected by a call site.
    pub(crate) fn targets(
        &self,
        site: CallSite<'_>,
    ) -> Result<ResolvedCallTargets, PackageStoreError> {
        match site {
            CallSite::Function { callee } => self.function_targets(callee),
            CallSite::Method(site) => self.lookup_method(site),
        }
    }

    /// Convert resolved callee declarations into callable function targets.
    fn function_targets(&self, callee: ExprId) -> Result<ResolvedCallTargets, PackageStoreError> {
        let mut targets = ResolvedCallTargets::new();
        let callee_data = self.context.body().expr_unchecked(callee);
        let BodyResolution::Declarations(declarations) =
            self.context.body().expr_resolution(callee)
        else {
            return Ok(targets);
        };

        for declaration in declarations {
            let Some(function) = self.declaration_function(*declaration)? else {
                continue;
            };
            targets.push(ResolvedCallTarget::function_call(
                function,
                callee_data.scope,
                Self::explicit_callee_generic_args(callee_data),
            ));
        }
        Ok(targets)
    }

    /// Convert receiver method lookup into callable method targets.
    fn lookup_method(
        &self,
        site: MethodCallSite<'_>,
    ) -> Result<ResolvedCallTargets, PackageStoreError> {
        let receiver_ty = self.context.body().expr_ty_unchecked(site.receiver);
        let mut targets = ResolvedCallTargets::new();

        for candidate in self
            .context
            .methods()
            .named_method_candidates_for_ty(receiver_ty, site.name)?
        {
            targets.push(ResolvedCallTarget::method_call(
                candidate.function(),
                site.scope,
                site.explicit_args,
                MethodReceiver {
                    self_ty: candidate.receiver_ty().clone(),
                    subst: candidate.subst().clone(),
                },
            ));
        }

        Ok(targets)
    }

    /// Keep only declarations that name functions.
    fn declaration_function(
        &self,
        declaration: DeclarationRef,
    ) -> Result<Option<FunctionRef>, PackageStoreError> {
        match declaration {
            DeclarationRef::LocalDef(local_def) => self.local_def_function(DefId::Local(local_def)),
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

    /// Read turbofish args from a path callee.
    fn explicit_callee_generic_args(callee_data: &ExprData) -> &[ItemGenericArg] {
        // A normal call expression has a callee expression, so `make::<T>()` and
        // `Type::build::<T>()` carry call generics on the final callee path segment. Method calls
        // are a different ExprKind and store their method-name generics directly.
        match &callee_data.kind {
            ExprKind::Path { path } => path.last_segment_angle_args().unwrap_or(&[]),
            _ => &[],
        }
    }

    /// Convert a body-local def into a function item when possible.
    fn local_def_function(&self, def: DefId) -> Result<Option<FunctionRef>, PackageStoreError> {
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

impl<'call, 'query, D, I> CallSignature<'call, 'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    /// Return declared parameter types for written call arguments.
    pub(crate) fn param_tys(&self) -> Result<Vec<Ty>, PackageStoreError> {
        let item_query = self.query.context.item_query();
        let Some(function_data) = item_query.function_data(self.target.function)? else {
            return Ok(Vec::new());
        };
        let subst = self.base_subst(function_data.signature.generics())?;
        let param_resolver = self
            .query
            .context
            .type_refs(TypeRefUseSite::Function(self.target.function))
            .with_subst(&subst);

        // Keep one expected type per written argument. Missing parameter annotations are not
        // useful evidence, but `Unknown` preserves arity so the caller can still zip safely.
        function_data
            .signature
            .params()
            .iter()
            .skip(self.target.receiver.first_written_param_idx())
            .map(|param| {
                let Some(param_ty) = &param.ty else {
                    return Ok(Ty::Unknown);
                };

                param_resolver.resolve(param_ty)
            })
            .collect()
    }

    /// Return the projected call result type.
    pub(crate) fn return_ty(&self, args: &[ExprId]) -> Result<Ty, PackageStoreError> {
        let item_query = self.query.context.item_query();
        let Some(function_data) = item_query.function_data(self.target.function)? else {
            return Ok(Ty::Unknown);
        };
        let Some(ret_ty) = function_data.signature.ret_ty() else {
            return Ok(Ty::Unit);
        };

        let mut subst = self.base_subst(function_data.signature.generics())?;
        let arg_tys = args
            .iter()
            .map(|arg| self.query.context.body().expr_ty_unchecked(*arg).clone())
            .collect::<Vec<_>>();
        subst.extend(
            CallArgInference::new(
                function_data.signature.generics(),
                function_data.signature.params(),
                &arg_tys,
                self.target.receiver.arg_mapping(),
                &subst,
            )
            .infer(),
        );

        self.project_return(&subst, ret_ty)
    }

    /// Return whether the call result should become a type variable.
    pub(crate) fn can_seed_return_inference(&self) -> Result<bool, PackageStoreError> {
        if !self.target.explicit_args().is_empty() {
            return Ok(false);
        }

        let Some(function_data) = self
            .query
            .context
            .item_query()
            .function_data(self.target.function)?
        else {
            return Ok(false);
        };
        let Some(generics) = function_data.signature.generics() else {
            return Ok(false);
        };
        let Some(ret_ty) = function_data.signature.ret_ty() else {
            return Ok(false);
        };
        let Some(ret_name) = ret_ty.type_param_name() else {
            return Ok(false);
        };

        // `fn make<T>() -> T` has no concrete `Ty` before expected-type constraints run, but
        // inference can preserve the return as `?T` and let the outer expression solve it.
        Ok(generics
            .types
            .iter()
            .any(|param| param.name.as_str() == ret_name.as_str()))
    }

    /// Combine receiver, shadow, and explicit generic substitutions.
    fn base_subst(&self, generics: Option<&GenericParams>) -> Result<TypeSubst, PackageStoreError> {
        let mut subst = self.target.receiver.base_subst();
        subst.extend(function_generic_shadow_subst(generics));
        subst.extend(self.explicit_subst(generics)?);
        Ok(subst)
    }

    /// Bind written function generics at the call-site scope.
    fn explicit_subst(
        &self,
        generics: Option<&GenericParams>,
    ) -> Result<TypeSubst, PackageStoreError> {
        let Some(generics) = generics else {
            return Ok(TypeSubst::new());
        };
        if self.target.explicit_args.is_empty() {
            return Ok(TypeSubst::new());
        }

        // Function turbofish arguments are supplied at the call site, so names inside them must
        // resolve from the body scope where the call was written.
        self.query.context.generics().subst_for_explicit_args(
            generics,
            &self.target.explicit_args,
            TypeRefUseSite::Scope(self.target.site_scope),
        )
    }

    /// Resolve the declared return type after call-specific substitutions.
    fn project_return(&self, subst: &TypeSubst, ret_ty: &TypeRef) -> Result<Ty, PackageStoreError> {
        if ret_ty.is_self_type() {
            return Ok(match self.target.receiver.self_ty() {
                Some(self_ty) => self_ty,
                None => self
                    .query
                    .context
                    .functions()
                    .self_nominal_ty(self.target.function)?
                    .into_self_ty(),
            });
        }

        self.query
            .context
            .type_refs(TypeRefUseSite::Function(self.target.function))
            .with_subst(subst)
            .resolve(ret_ty)
    }
}
