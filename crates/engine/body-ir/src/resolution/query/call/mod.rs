//! Function and method call resolution.

mod signature;
mod target;

use rg_def_map::DefMapSource;
use rg_ir_model::{DefId, ExprId, FunctionRef, ScopeId, SemanticItemRef, identity::DeclarationRef};
use rg_item_tree::GenericArg as ItemGenericArg;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::{Ty, inference::InferenceTable};

use crate::body::facts::BodyResolution;
use crate::{
    body::{ExprData, ExprKind},
    resolution::BodyResolutionContext,
};

use self::target::{CallSelf, ResolvedCallTargets};
use super::BodyCallableCandidate;

pub(crate) use self::{
    signature::{CallProjection, CallSignature},
    target::ResolvedCallTarget,
};

/// Method-call syntax facts needed for method lookup.
struct MethodCallSite<'a> {
    name: &'a str,
    explicit_args: &'a [ItemGenericArg],
    scope: ScopeId,
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

        for candidate in self.context.methods().named_method_candidates_for_ty(
            site.scope,
            receiver_ty,
            site.name,
            table,
        )? {
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
