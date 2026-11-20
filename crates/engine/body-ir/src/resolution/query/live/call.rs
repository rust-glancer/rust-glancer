//! Call lookup retains the trial context that established a receiver's applicability.
//!
//! For `value.method()`, compare the method's self parameter with the receiver adjustments.
//! An impl for `Widget` can take `&Widget`, while an impl for `&Widget` can take `&&Widget`.
//! The first matching adjustment wins; inherent methods take precedence at the same adjustment.
//! The chosen target retains its receiver bindings so its signature uses the same variables as
//! the body.

use rg_def_map::DefMapSource;
use rg_ir_model::{
    ExprId, FunctionRef, ItemOwner, Mutability, ScopeId, SemanticItemRef, identity::DeclarationRef,
};
use rg_item_tree::{ParamKind, SelfParamKind};
use rg_semantic_ir::SemanticItemSource;
use rg_source::ReadError;
use rg_ty::{
    lowering::{TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery},
    solver::{InferenceSubstitution, InferenceTable, Outcome, Ty},
};

use super::{FunctionLookup, LiveBodyQuery, member::MemberCandidate};
use crate::{
    BodyAssociatedPathPrefix,
    body::{ExprKind, facts::BodyResolution},
};

/// A function candidate together with any receiver evidence found while considering it.
/// Member lookup keeps that evidence in a separate table so candidates cannot constrain one
/// another. A directly resolved function needs no trial; its signature uses the body's table.
pub(crate) struct LiveCallTarget<'s> {
    pub function: FunctionRef,
    pub subst: InferenceSubstitution<'s>,
    // For a dot call, this includes the borrow or dereference that matched the self parameter.
    pub receiver: Option<Ty<'s>>,
    pub table: Option<InferenceTable<'s>>,
    // Lookup can retain a declaration for navigation even when its trial cannot supply types.
    can_infer: bool,
}

impl<'s> LiveCallTarget<'s> {
    fn from_member(candidate: MemberCandidate<'s>) -> Self {
        Self {
            function: candidate.function,
            subst: candidate.subst,
            receiver: None,
            table: Some(candidate.table),
            // A possible proof can learn from call arguments later. Missing callback data
            // still leaves a navigation candidate, but cannot supply inference evidence.
            can_infer: matches!(candidate.outcome, Outcome::Proven | Outcome::Ambiguous),
        }
    }

    pub(crate) fn unique_usable(targets: Vec<Self>) -> Option<Self> {
        let mut usable = targets.into_iter().filter(|target| target.can_infer);
        match (usable.next(), usable.next()) {
            (Some(target), None) => Some(target),
            _ => None,
        }
    }
}

impl<'query, D, I> LiveBodyQuery<'query, D, I>
where
    D: DefMapSource + Copy,
    I: SemanticItemSource<'query> + Copy,
{
    /// Find the best dot-call candidates in receiver adjustment order. Ties remain useful
    /// for navigation even when inference cannot choose a signature.
    pub(crate) fn method_targets<'s>(
        &self,
        call: ExprId,
        receiver: Ty<'s>,
        table: &InferenceTable<'s>,
    ) -> Result<Vec<LiveCallTarget<'s>>, ReadError> {
        let data = self.context.body().expr_unchecked(call);
        let ExprKind::MethodCall { method_name, .. } = &data.kind else {
            unreachable!("method lookup owns a dot call")
        };
        // 1. Build two lists with different jobs. `self_types` gives us the types to
        // inspect for member declarations, following references and Deref, with an
        // array-to-slice adjustment. The traversal bounds the chain and stops repeats.
        // `receivers` gives the order in which a method's self parameter may match:
        // T, &T, &mut T, then the same three forms for the next dereference step.
        let cx = table.interner();
        let self_types = table.method_receivers(receiver).collect::<Vec<_>>();
        let receivers = self_types
            .iter()
            .flat_map(|&ty| {
                [
                    ty,
                    cx.reference(Mutability::Shared, ty),
                    cx.reference(Mutability::Mutable, ty),
                ]
            })
            .collect::<Vec<_>>();
        let items = self.context.item_query();
        let mut selected = Vec::new();
        let mut best_priority = None;
        // 2. Collect methods for each possible Self, keeping both inherent and trait
        // methods. Finding a method here is not enough to choose it: its self parameter
        // may need another borrow, while a method found later may match directly.
        for self_ty in self_types {
            for mut target in self
                .function_candidates(
                    data.scope,
                    self_ty,
                    FunctionLookup::Method(method_name),
                    table,
                )?
                .into_iter()
                .map(LiveCallTarget::from_member)
            {
                let Some(function) = items.function_data(target.function)? else {
                    continue;
                };
                let Some(param) = function.signature.params().first() else {
                    continue;
                };
                // 3. Work out the type this method actually accepts as its receiver.
                // With Self = Widget, &self accepts &Widget. With Self = &Widget,
                // the same &self spelling accepts &&Widget. A typed receiver such as
                // `self: &Self` gets its type from the signature, with this candidate's
                // generic substitutions applied to the types written in that parameter.
                let parameter_ty = match param.kind {
                    ParamKind::SelfParam(SelfParamKind::Value) => self_ty,
                    ParamKind::SelfParam(SelfParamKind::Reference { mutability }) => {
                        cx.reference(mutability, self_ty)
                    }
                    ParamKind::SelfParam(SelfParamKind::Explicit) => {
                        let Some(signature) = cx.function_signature(target.function) else {
                            continue;
                        };
                        let Some(param) = signature.params.first() else {
                            continue;
                        };
                        target.subst.apply(cx, *param)
                    }
                    ParamKind::Normal => continue,
                };
                // 4. Find the earliest receiver adjustment that matches that parameter.
                // For a value of type &Widget, a parameter of &Widget matches before
                // one of &&Widget. Matching can also learn generic arguments, so do it
                // in this candidate's own trial. Failed matches roll back their bindings;
                // a successful match keeps its bindings without changing other candidates.
                let trial = target.table.as_ref().expect("member candidate has a trial");
                let Some((position, &receiver)) = receivers
                    .iter()
                    .enumerate()
                    .find(|(_, receiver)| trial.try_unify(**receiver, parameter_ty).is_ok())
                else {
                    continue;
                };

                // 5. Compare this match with the best ones seen so far. Receiver order
                // comes first; inherent methods win only at the same receiver position.
                // For a value of type Widget, a trait's &self method therefore beats an
                // inherent &mut self method. A better match replaces the saved candidates;
                // an equal match joins them so we do not resolve ambiguity by search order.
                let priority = (position, !matches!(function.owner, ItemOwner::Impl(_)));
                if best_priority.is_some_and(|best| priority > best) {
                    continue;
                }
                if best_priority != Some(priority) {
                    selected.clear();
                    best_priority = Some(priority);
                }
                // 6. Carry the matched receiver along with the candidate's bindings.
                // Later signature checking needs &Widget for an &self parameter, even
                // when the impl's Self is Widget. Keeping only Self would lose the borrow.
                target.receiver = Some(receiver);
                selected.push(target);
            }
        }
        Ok(selected)
    }

    /// Resolve a path naming a function item, whether or not the item is called here.
    pub(crate) fn associated_function_targets<'s>(
        &self,
        scope: ScopeId,
        prefix: &BodyAssociatedPathPrefix,
        name: &str,
        table: &InferenceTable<'s>,
    ) -> Result<Vec<LiveCallTarget<'s>>, ReadError> {
        // `Widget::read` supplies Self = Widget; `<Widget as Read<u8>>::read` also fixes
        // the trait and its arguments. All of these constraints come from the path.
        let (receiver, qualification) = match prefix {
            BodyAssociatedPathPrefix::Type(ty) => (self.type_ref(scope, ty, table)?, None),
            BodyAssociatedPathPrefix::QualifiedTrait { self_ty, trait_ref } => {
                let paths = self.context.item_paths();
                let lowering = TypeLoweringQuery::new(&paths, &self.context);
                let mut session = lowering.session(
                    table.interner(),
                    TypeLoweringEnv::new(
                        self.context.body().owner().generic_def(),
                        TypeLoweringAnchor::Scope(scope),
                    ),
                )?;
                let receiver = session.lower_type_ref_with_inference(self_ty, table)?;
                let qualification = session
                    .lower_trait_ref(trait_ref, receiver)?
                    .map(|tr| tr.application);
                (receiver, qualification)
            }
        };
        if receiver.is_unknown() && qualification.is_none() {
            return Ok(Vec::new());
        }

        // A prefix such as `Container<_>` can leave holes for the item's use to fill.
        let receiver = table.instantiate_nested_unknowns(receiver);
        Ok(self
            .function_candidates(
                scope,
                receiver,
                FunctionLookup::Associated {
                    name,
                    qualification,
                },
                table,
            )?
            .into_iter()
            .map(LiveCallTarget::from_member)
            .collect())
    }

    pub(crate) fn declared_function_targets<'s>(
        &self,
        resolution: &BodyResolution,
    ) -> Result<Vec<LiveCallTarget<'s>>, ReadError> {
        // An ordinary function path, or an associated path with no target above, can use
        // its resolved declarations. There is no adjusted receiver or receiver trial here.
        let mut targets = Vec::new();
        if let BodyResolution::Declarations(declarations) = resolution {
            for declaration in declarations {
                if let Some(function) = self.declaration_function(*declaration)? {
                    targets.push(LiveCallTarget {
                        function,
                        subst: InferenceSubstitution::new(),
                        receiver: None,
                        table: None,
                        can_infer: true,
                    });
                }
            }
        }
        Ok(targets)
    }

    /// Keep only declarations that name functions.
    fn declaration_function(
        &self,
        declaration: DeclarationRef,
    ) -> Result<Option<FunctionRef>, ReadError> {
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
