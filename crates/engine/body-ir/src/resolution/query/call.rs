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

use super::associated_item::BodyAssociatedFunctionCandidate;

/// Function target selected by call syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedCallTarget {
    function: FunctionRef,
    explicit_args: Vec<ItemGenericArg>,
    site_scope: ScopeId,
    self_source: CallSelfSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CallSelfSource {
    None,
    TypePrefix(CallSelf),
    Receiver(CallSelf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CallSelf {
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
            self_source: CallSelfSource::None,
        }
    }

    /// Build target data for a method call with receiver facts.
    fn method_call(
        function: FunctionRef,
        site_scope: ScopeId,
        explicit_args: &[ItemGenericArg],
        receiver: CallSelf,
    ) -> Self {
        Self {
            function,
            explicit_args: explicit_args.to_vec(),
            site_scope,
            self_source: CallSelfSource::Receiver(receiver),
        }
    }

    /// Build target data for an associated function call with selected `Self`.
    fn associated_function_call(
        function: FunctionRef,
        site_scope: ScopeId,
        explicit_args: &[ItemGenericArg],
        self_context: CallSelf,
    ) -> Self {
        Self {
            function,
            explicit_args: explicit_args.to_vec(),
            site_scope,
            self_source: CallSelfSource::TypePrefix(self_context),
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

    /// Return the first signature param matched by written call args.
    pub(crate) fn first_written_param_idx(&self) -> usize {
        self.self_source.first_written_param_idx()
    }

    /// Return whether this static call was selected through a type prefix.
    pub(crate) fn has_type_prefix_self_source(&self) -> bool {
        matches!(self.self_source, CallSelfSource::TypePrefix(_))
    }
}

impl CallSelfSource {
    /// Choose how written arguments line up with declared params.
    fn arg_mapping(&self) -> CallArgMapping {
        match self {
            Self::None => CallArgMapping::FunctionCall,
            Self::TypePrefix(_) => CallArgMapping::FunctionCall,
            Self::Receiver(_) => CallArgMapping::MethodCall,
        }
    }

    /// Skip implicit receiver params when projecting written arguments.
    fn first_written_param_idx(&self) -> usize {
        match self {
            Self::None => 0,
            Self::TypePrefix(_) => 0,
            Self::Receiver(_) => 1,
        }
    }

    /// Start signature projection with receiver-derived substitutions.
    fn base_subst(&self) -> TypeSubst {
        match self {
            Self::None => TypeSubst::new(),
            Self::TypePrefix(self_context) | Self::Receiver(self_context) => {
                self_context.subst.clone()
            }
        }
    }

    /// Return concrete `Self` when this call was selected through a receiver or type prefix.
    fn self_ty(&self) -> Option<Ty> {
        match self {
            Self::None => None,
            Self::TypePrefix(self_context) | Self::Receiver(self_context) => {
                Some(self_context.self_ty.clone())
            }
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
            let projection = calls.signature(target).project(args)?;
            return_tys.push(projection.return_ty().clone());
        }

        Ok(return_tys.into_ty())
    }

    /// Add one target, preserving uniqueness.
    fn push(&mut self, target: ResolvedCallTarget) {
        self.targets.push(target);
    }

    /// Return the target only when lookup is unambiguous.
    fn single(&self) -> Option<ResolvedCallTarget> {
        self.targets.as_one().cloned()
    }
}

/// Projects a selected call target into parameter and return types.
pub(crate) struct CallSignature<'call, 'query, D, I> {
    query: &'call BodyCallQuery<'query, D, I>,
    target: &'call ResolvedCallTarget,
}

/// Signature facts projected for one selected call target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CallProjection {
    written_param_tys: Vec<Ty>,
    return_ty: Ty,
    declared_return_ty: Option<TypeRef>,
    function_generics: Option<GenericParams>,
    explicit_args: Vec<ItemGenericArg>,
    selected_self_ty: Option<Ty>,
}

impl CallProjection {
    fn unknown(explicit_args: &[ItemGenericArg]) -> Self {
        Self {
            written_param_tys: Vec::new(),
            return_ty: Ty::Unknown,
            declared_return_ty: None,
            function_generics: None,
            explicit_args: explicit_args.to_vec(),
            selected_self_ty: None,
        }
    }

    /// Return parameter types for arguments written at the call site.
    pub(crate) fn written_param_tys(&self) -> &[Ty] {
        &self.written_param_tys
    }

    /// Return the projected call result type.
    pub(crate) fn return_ty(&self) -> &Ty {
        &self.return_ty
    }

    /// Return the declared return type syntax.
    pub(crate) fn declared_return_ty(&self) -> Option<&TypeRef> {
        self.declared_return_ty.as_ref()
    }

    /// Return the function generics visible in the signature.
    pub(crate) fn function_generics(&self) -> Option<&GenericParams> {
        self.function_generics.as_ref()
    }

    /// Return explicit generic arguments written at the call site.
    pub(crate) fn explicit_args(&self) -> &[ItemGenericArg] {
        &self.explicit_args
    }

    /// Return the `Self` type that selected this associated function or method.
    pub(crate) fn selected_self_ty(&self) -> Option<&Ty> {
        self.selected_self_ty.as_ref()
    }
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
        let associated_targets = self.associated_function_targets(callee_data)?;
        if !associated_targets.is_empty() {
            return Ok(associated_targets);
        }

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

    /// Rebuild associated function targets with the typed path prefix preserved.
    fn associated_function_targets(
        &self,
        callee_data: &ExprData,
    ) -> Result<ResolvedCallTargets, PackageStoreError> {
        let mut targets = ResolvedCallTargets::new();
        let ExprKind::Path { path } = &callee_data.kind else {
            return Ok(targets);
        };
        for candidate in self
            .context
            .associated_items()
            .function_candidates_for_body_path(callee_data.scope, path)?
        {
            targets.push(Self::associated_function_target(callee_data, candidate));
        }

        Ok(targets)
    }

    fn associated_function_target(
        callee_data: &ExprData,
        candidate: BodyAssociatedFunctionCandidate,
    ) -> ResolvedCallTarget {
        ResolvedCallTarget::associated_function_call(
            candidate.function(),
            callee_data.scope,
            Self::explicit_callee_generic_args(callee_data),
            CallSelf {
                self_ty: candidate.self_ty().clone(),
                subst: candidate.subst().clone(),
            },
        )
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
                CallSelf {
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
    /// Project written parameter types and result type for this selected call.
    pub(crate) fn project(&self, args: &[ExprId]) -> Result<CallProjection, PackageStoreError> {
        let item_query = self.query.context.item_query();
        let Some(function_data) = item_query.function_data(self.target.function)? else {
            return Ok(CallProjection::unknown(self.target.explicit_args()));
        };
        let generics = function_data.signature.generics();
        let base_subst = self.base_subst(generics)?;
        let param_resolver = self
            .query
            .context
            .type_refs(TypeRefUseSite::Function(self.target.function))
            .with_subst(&base_subst);

        // Keep one expected type per written argument. Missing parameter annotations are not
        // useful evidence, but `Unknown` preserves arity so the caller can still zip safely.
        let written_param_tys = function_data
            .signature
            .params()
            .iter()
            .skip(self.target.self_source.first_written_param_idx())
            .map(|param| {
                let Some(param_ty) = &param.ty else {
                    return Ok(Ty::Unknown);
                };

                param_resolver.resolve(param_ty)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut return_subst = base_subst.clone();
        let arg_tys = args
            .iter()
            .map(|arg| self.query.context.body().expr_ty_unchecked(*arg).clone())
            .collect::<Vec<_>>();
        return_subst.extend(
            CallArgInference::new(
                generics,
                function_data.signature.params(),
                &arg_tys,
                self.target.self_source.arg_mapping(),
                &return_subst,
            )
            .infer(),
        );

        let declared_return_ty = function_data.signature.ret_ty().cloned();
        let return_ty = match declared_return_ty.as_ref() {
            Some(ret_ty) => self.project_return(&return_subst, ret_ty)?,
            None => Ty::Unit,
        };

        Ok(CallProjection {
            written_param_tys,
            return_ty,
            declared_return_ty,
            function_generics: generics.cloned(),
            explicit_args: self.target.explicit_args().to_vec(),
            selected_self_ty: self.target.self_source.self_ty(),
        })
    }

    /// Combine receiver, shadow, and explicit generic substitutions.
    fn base_subst(&self, generics: Option<&GenericParams>) -> Result<TypeSubst, PackageStoreError> {
        let mut subst = self.target.self_source.base_subst();
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
            return Ok(match self.target.self_source.self_ty() {
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
