//! Call lookup retains the trial context that established a receiver's applicability.
//!
//! For `value.method()`, try receiver adjustments in order. `&&Widget` can expose candidates at
//! `&&Widget`, `&Widget`, or `Widget`; the first depth with candidates wins. Within that depth,
//! inherent methods take precedence over methods from visible traits. The chosen target retains
//! its receiver bindings so its signature uses the same variables as the body.

use rg_def_map::DefMapSource;
use rg_ir_model::{ExprId, FunctionRef, ScopeId, SemanticItemRef, identity::DeclarationRef};
use rg_item_tree::GenericArg as ItemGenericArg;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::{
    lowering::{TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery},
    solver::{InferenceSubstitution, InferenceTable, Outcome, Ty},
};

use super::{FunctionLookup, LiveBodyQuery};
use crate::{
    BodyAssociatedPathPrefix,
    body::{ExprKind, facts::BodyResolution},
};

/// A function candidate together with any receiver evidence found while considering it.
/// Member lookup keeps that evidence in a separate table so candidates cannot constrain one
/// another. A directly resolved function needs no trial; its signature uses the body's table.
pub(crate) struct LiveCallTarget<'s> {
    pub function: FunctionRef,
    pub explicit_args: Vec<ItemGenericArg>,
    pub scope: ScopeId,
    pub subst: InferenceSubstitution<'s>,
    pub receiver: Option<Ty<'s>>,
    pub first_written: usize,
    pub table: Option<InferenceTable<'s>>,
    // Lookup can retain a declaration for navigation even when its trial cannot supply types.
    pub can_infer: bool,
}

impl<'query, D, I> LiveBodyQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    /// Collect call targets, stopping at the first matching receiver adjustment for a method.
    /// Several candidates remain useful for navigation; call inference requires a unique usable
    /// target before it can connect a signature to the body's argument and result variables.
    pub(crate) fn call_targets<'s>(
        &self,
        call: ExprId,
        resolution: Option<&BodyResolution>,
        receiver: Option<Ty<'s>>,
        table: &InferenceTable<'s>,
    ) -> Result<Vec<LiveCallTarget<'s>>, PackageStoreError> {
        let data = self.context.body().expr_unchecked(call);
        match &data.kind {
            ExprKind::MethodCall {
                method_name,
                generic_args,
                ..
            } => {
                let Some(receiver) = receiver else {
                    return Ok(Vec::new());
                };
                for receiver in table.method_receivers(receiver) {
                    let candidates = self.member_targets(
                        data.scope,
                        receiver,
                        FunctionLookup::Method(method_name),
                        generic_args,
                        table,
                    )?;
                    if !candidates.is_empty() {
                        return Ok(candidates);
                    }
                }
                Ok(Vec::new())
            }
            ExprKind::Call {
                callee: Some(callee),
                ..
            } => {
                let callee = self.context.body().expr_unchecked(*callee);
                let explicit = match &callee.kind {
                    ExprKind::Path { path } => path.last_segment_angle_args().unwrap_or(&[]),
                    _ => &[],
                };
                if let ExprKind::Path { path } = &callee.kind
                    && let Some((prefix, name)) = path.split_associated_item_prefix_name()
                {
                    let (receiver, qualification) = match prefix {
                        BodyAssociatedPathPrefix::Type(ty) => {
                            (self.type_ref(callee.scope, &ty, table)?, None)
                        }
                        BodyAssociatedPathPrefix::QualifiedTrait { self_ty, trait_ref } => {
                            let paths = self.context.item_paths();
                            let lowering = TypeLoweringQuery::new(&paths, &self.context);
                            let mut session = lowering.session(
                                table.interner(),
                                TypeLoweringEnv::new(
                                    self.context.body().owner().generic_def(),
                                    TypeLoweringAnchor::Scope(callee.scope),
                                ),
                            )?;
                            let receiver =
                                session.lower_type_ref_with_inference(&self_ty, table)?;
                            let qualification = session
                                .lower_trait_ref(&trait_ref, receiver)?
                                .map(|tr| tr.application);
                            (receiver, qualification)
                        }
                    };
                    if !receiver.is_unknown() || qualification.is_some() {
                        let receiver = table.instantiate_nested_unknowns(receiver);
                        let targets = self.member_targets(
                            callee.scope,
                            receiver,
                            FunctionLookup::Associated {
                                name,
                                qualification,
                            },
                            explicit,
                            table,
                        )?;
                        if !targets.is_empty() {
                            return Ok(targets);
                        }
                    }
                }
                let mut targets = Vec::new();
                if let Some(BodyResolution::Declarations(declarations)) = resolution {
                    for declaration in declarations {
                        if let Some(function) = self.declaration_function(*declaration)? {
                            targets.push(LiveCallTarget {
                                function,
                                explicit_args: explicit.to_vec(),
                                scope: callee.scope,
                                subst: InferenceSubstitution::new(),
                                receiver: None,
                                first_written: 0,
                                table: None,
                                can_infer: true,
                            });
                        }
                    }
                }
                Ok(targets)
            }
            _ => Ok(Vec::new()),
        }
    }

    fn member_targets<'s>(
        &self,
        scope: ScopeId,
        receiver: Ty<'s>,
        lookup: FunctionLookup<'_, 's>,
        explicit: &[ItemGenericArg],
        table: &InferenceTable<'s>,
    ) -> Result<Vec<LiveCallTarget<'s>>, PackageStoreError> {
        Ok(self
            .function_candidates(scope, receiver, lookup, table)?
            .into_iter()
            .map(|candidate| LiveCallTarget {
                function: candidate.function,
                explicit_args: explicit.to_vec(),
                scope,
                subst: candidate.subst,
                receiver: Some(receiver),
                first_written: usize::from(matches!(lookup, FunctionLookup::Method(_))),
                table: Some(candidate.table),
                // A possible proof can learn from call arguments later. Missing callback data
                // still leaves a navigation candidate, but cannot supply inference evidence.
                can_infer: matches!(candidate.outcome, Outcome::Proven | Outcome::Ambiguous),
            })
            .collect())
    }

    /// Keep only declarations that name functions.
    fn declaration_function(
        &self,
        declaration: DeclarationRef,
    ) -> Result<Option<FunctionRef>, PackageStoreError> {
        match declaration {
            DeclarationRef::LocalDef(local_def) => Ok(
                match self
                    .context
                    .item_query()
                    .semantic_item_for_local_def(local_def)?
                {
                    Some(SemanticItemRef::Function(function)) => Some(function),
                    Some(_) | None => None,
                },
            ),
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
}
