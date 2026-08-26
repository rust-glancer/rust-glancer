//! Function and method call resolution.

use rg_def_map::DefMapSource;
use rg_ir_model::{
    DefId, ExprId, FunctionRef, GenericDefRef, GenericParamRef, ScopeId, SemanticItemRef,
    identity::DeclarationRef,
};
use rg_item_tree::{FunctionQualifiers, GenericArg as ItemGenericArg};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{GenericParamSource, Generics, ItemStoreSource};
use rg_std::{ExpectedUnique, UniqueVec};
use rg_ty::{
    CallableSignature, GenericArg, Substitution, TraitSelection, Ty, inference::InferenceTable,
};

use crate::resolution::BodyResolutionContext;
use crate::{
    ir::resolved::BodyResolution,
    ir::{ExprData, ExprKind},
};

use super::BodyCallableCandidate;

/// Semantic function selected for one written call before body-local inference.
///
/// The target retains explicit generic syntax and the call-site scope so type arguments can be
/// lowered later against the correct body context. Receiver or type-prefix evidence is kept
/// separately from function-owned generics. Trait candidates also retain their trial selection so
/// inference can commit its table only after lookup finds one definite target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedCallTarget {
    function: FunctionRef,
    explicit_args: Vec<ItemGenericArg>,
    site_scope: ScopeId,
    self_source: CallSelfSource,
    trait_selection: Option<TraitSelection>,
}

/// How `Self` entered a selected call and whether syntax supplied an implicit receiver argument.
///
/// `Type::make(value)` contributes a `Self` substitution but its written arguments still begin at
/// signature parameter zero. `value.method(arg)` contributes the same substitution and consumes
/// parameter zero as the implicit receiver.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CallSelfSource {
    None,
    TypePrefix(CallSelf),
    Receiver(CallSelf),
}

/// Concrete `Self` evidence recovered together with its owner-scoped substitution.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CallSelf {
    self_ty: Ty,
    subst: Substitution,
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
            trait_selection: None,
        }
    }

    /// Build target data for a method call with receiver facts.
    fn method_call(
        function: FunctionRef,
        site_scope: ScopeId,
        explicit_args: &[ItemGenericArg],
        receiver: CallSelf,
        trait_selection: Option<TraitSelection>,
    ) -> Self {
        Self {
            function,
            explicit_args: explicit_args.to_vec(),
            site_scope,
            self_source: CallSelfSource::Receiver(receiver),
            trait_selection,
        }
    }

    /// Build target data for an associated function call with selected `Self`.
    fn associated_function_call(
        function: FunctionRef,
        site_scope: ScopeId,
        explicit_args: &[ItemGenericArg],
        self_context: CallSelf,
        trait_selection: Option<TraitSelection>,
    ) -> Self {
        Self {
            function,
            explicit_args: explicit_args.to_vec(),
            site_scope,
            self_source: CallSelfSource::TypePrefix(self_context),
            trait_selection,
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

    /// Return the body scope where explicit call arguments were written.
    pub(crate) fn site_scope(&self) -> ScopeId {
        self.site_scope
    }

    /// Return the first signature param matched by written call args.
    pub(crate) fn first_written_param_idx(&self) -> usize {
        self.self_source.first_written_param_idx()
    }

    /// Return trait-selection evidence whose table is committed with a definite target.
    pub(crate) fn trait_selection(&self) -> Option<&TraitSelection> {
        self.trait_selection.as_ref()
    }
}

impl CallSelfSource {
    /// Skip implicit receiver params when projecting written arguments.
    fn first_written_param_idx(&self) -> usize {
        match self {
            Self::None => 0,
            Self::TypePrefix(_) => 0,
            Self::Receiver(_) => 1,
        }
    }

    /// Start signature projection with receiver-derived substitutions.
    fn base_subst(&self) -> Substitution {
        match self {
            Self::None => Substitution::new(),
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

/// Method-call syntax facts needed for method lookup.
struct MethodCallSite<'a> {
    name: &'a str,
    explicit_args: &'a [ItemGenericArg],
    scope: ScopeId,
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

    /// Add one target, preserving uniqueness.
    fn push(&mut self, target: ResolvedCallTarget) {
        self.targets.push(target);
    }

    /// Return the unique target whose trait predicates were fully proved.
    fn single_proven(&self) -> Option<ResolvedCallTarget> {
        let mut target = ExpectedUnique::new();
        for candidate in &self.targets {
            // Ordinary and inherent functions need no trait proof. Trait functions must have a
            // definite selection; `Maybe` remains useful to editor lookup but cannot own call
            // inference or associated projection facts.
            if candidate.trait_selection.as_ref().is_none_or(|selection| {
                selection.applicability == rg_ir_model::TraitApplicability::Yes
            }) {
                target.push(candidate.clone());
            }
        }
        target.into_option()
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

    /// Return the selected target, preferring a live inference receiver for method calls.
    pub(crate) fn target_with_receiver_ty(
        &self,
        call: ExprId,
        receiver_ty: Option<&Ty>,
        table: &InferenceTable,
    ) -> Result<Option<ResolvedCallTarget>, PackageStoreError> {
        let expr_data = self.context.body().expr_unchecked(call);
        let targets = match &expr_data.kind {
            ExprKind::Call {
                callee: Some(callee),
                ..
            } => self.function_targets(*callee, table)?,
            ExprKind::Call { callee: None, .. } => return Ok(None),
            ExprKind::MethodCall {
                receiver: Some(receiver),
                method_name,
                generic_args,
                ..
            } => {
                let site = MethodCallSite {
                    name: method_name,
                    explicit_args: generic_args,
                    scope: expr_data.scope,
                };
                let receiver_ty = receiver_ty
                    .unwrap_or_else(|| self.context.query_body().expr_ty_unchecked(*receiver));
                self.lookup_method_for_ty(site, receiver_ty, table)?
            }
            ExprKind::MethodCall { receiver: None, .. } => return Ok(None),
            _ => return Ok(None),
        };

        Ok(targets.single_proven())
    }

    /// Resolve a method-call expression from a receiver type learned during body inference.
    pub(crate) fn method_targets_with_receiver_ty(
        &self,
        call: ExprId,
        receiver_ty: &Ty,
        table: &InferenceTable,
    ) -> Result<ResolvedCallTargets, PackageStoreError> {
        let expr_data = self.context.body().expr_unchecked(call);
        let ExprKind::MethodCall {
            receiver: Some(_),
            method_name,
            generic_args,
            ..
        } = &expr_data.kind
        else {
            return Ok(ResolvedCallTargets::new());
        };

        self.lookup_method_for_ty(
            MethodCallSite {
                name: method_name,
                explicit_args: generic_args,
                scope: expr_data.scope,
            },
            receiver_ty,
            table,
        )
    }

    /// Convert resolved callee declarations into callable function targets.
    fn function_targets(
        &self,
        callee: ExprId,
        table: &InferenceTable,
    ) -> Result<ResolvedCallTargets, PackageStoreError> {
        let mut targets = ResolvedCallTargets::new();
        let callee_data = self.context.body().expr_unchecked(callee);
        let associated_targets = self.associated_function_targets(callee_data, table)?;
        if !associated_targets.is_empty() {
            return Ok(associated_targets);
        }

        let BodyResolution::Declarations(declarations) =
            self.context.query_body().expr_resolution_unchecked(callee)
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
        table: &InferenceTable,
    ) -> Result<ResolvedCallTargets, PackageStoreError> {
        let mut targets = ResolvedCallTargets::new();
        let ExprKind::Path { path } = &callee_data.kind else {
            return Ok(targets);
        };
        for candidate in self
            .context
            .associated_items()
            .function_candidates_for_body_path(callee_data.scope, path, table)?
        {
            targets.push(Self::associated_function_target(callee_data, candidate));
        }

        Ok(targets)
    }

    fn associated_function_target(
        callee_data: &ExprData,
        candidate: BodyCallableCandidate,
    ) -> ResolvedCallTarget {
        ResolvedCallTarget::associated_function_call(
            candidate.function(),
            callee_data.scope,
            Self::explicit_callee_generic_args(callee_data),
            CallSelf {
                self_ty: candidate.receiver_ty().clone(),
                subst: candidate.subst().clone(),
            },
            candidate.trait_selection().cloned(),
        )
    }

    /// Convert receiver method lookup into targets using the supplied semantic receiver fact.
    fn lookup_method_for_ty(
        &self,
        site: MethodCallSite<'_>,
        receiver_ty: &Ty,
        table: &InferenceTable,
    ) -> Result<ResolvedCallTargets, PackageStoreError> {
        let mut targets = ResolvedCallTargets::new();

        for candidate in
            self.context
                .methods()
                .named_method_candidates_for_ty(receiver_ty, site.name, table)?
        {
            targets.push(ResolvedCallTarget::method_call(
                candidate.function(),
                site.scope,
                site.explicit_args,
                CallSelf {
                    self_ty: candidate.receiver_ty().clone(),
                    subst: candidate.subst().clone(),
                },
                candidate.trait_selection().cloned(),
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
        let Some(signature) = self
            .query
            .context
            .signatures()
            .function(self.target.function)?
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
            .generics(GenericDefRef::Function(self.target.function))?;
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
        if self.target.explicit_args.is_empty() {
            return Ok(Substitution::new());
        }

        // Function turbofish arguments are supplied at the call site, so names inside them must
        // resolve from the body scope where the call was written.
        self.query.context.generics().subst_for_explicit_args(
            GenericDefRef::Function(self.target.function),
            &self.target.explicit_args,
            self.target.site_scope,
        )
    }
}
