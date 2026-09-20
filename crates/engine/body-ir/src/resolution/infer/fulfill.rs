//! Pending semantic operations have live inputs and a stable destination. Syntax is never queued:
//! retries only perform the lookup, projection, obligation, or coercion that could not finish earlier.

use anyhow::Context as _;
use rg_def_map::DefMapSource;
use rg_ir_model::{ExprId, ItemOwner, PatId, TraitDefRef};
use rg_item_tree::LangItem;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::ExpectedUnique;
use rg_ty::{ExpectedTyExt, GenericArgs, Ty, autoderef::AutoderefMode, trait_selection::TraitGoal};

use super::InferenceContext;
use crate::{
    ExprUnaryOp,
    body::{ExprKind, PatKind},
};

/// One unfinished operation and the input against which it last ran or was queued.
/// The operation reads live slots; the stored input is only a snapshot for deciding when to retry.
pub(super) struct Deferred {
    kind: DeferredKind,
    // `None` requests an attempt at the next checkpoint, even without an outer input change.
    input: Option<Ty>,
}

pub(super) enum DeferredKind {
    Call { call: ExprId },
    Member { expr: ExprId },
    Pattern { pat: PatId, expected: Ty },
    IteratorItem { iterable: ExprId, item: Ty },
    TryOutput { expr: ExprId },
    Operator { expr: ExprId },
    Coerce { expr: ExprId, expected: Ty },
    BranchResult { expr: ExprId, branches: Vec<Ty> },
}

impl<'query, D, I> InferenceContext<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    /// Apply an expression expectation without equating a possible `!` to the expected type.
    /// Other supported cases use ordinary unification, but an unfinished producer needs to
    /// determine its result before we can choose between these two cases.
    pub(super) fn coerce_expr_ty(&mut self, expr: ExprId, expected: &Ty) {
        if !self.try_coerce_expr_ty(expr, expected) {
            self.defer(
                DeferredKind::Coerce {
                    expr,
                    expected: expected.clone(),
                },
                None,
            );
        }
    }

    fn try_coerce_expr_ty(&mut self, expr: ExprId, expected: &Ty) -> bool {
        if matches!(expected, Ty::Unknown) {
            return true;
        }
        if !self.can_coerce_ty(&self.inference.expr_ty(expr)) {
            return false;
        }
        self.inference.constrain_expr_ty(expr, expected);
        true
    }

    /// Ordinary generic variables can learn from an expectation. Slots awaiting a lookup or
    /// projection need that operation's answer first: it may be `!`, which coerces without equality.
    /// Compare live roots so this also protects a pending result read through a binding or block.
    fn can_coerce_ty(&self, ty: &Ty) -> bool {
        let ty = self.inference.root_resolved_ty(ty);
        if !matches!(
            ty,
            Ty::InferVar {
                kind: rg_ty::inference::InferVarKind::Type,
                ..
            }
        ) {
            return true;
        }

        !self.deferred.iter().any(|operation| match &operation.kind {
            DeferredKind::Call { call } => self.inference.call_result_is_pending(*call, &ty),
            DeferredKind::Member { expr }
            | DeferredKind::TryOutput { expr }
            | DeferredKind::Operator { expr }
            | DeferredKind::BranchResult { expr, .. } => {
                self.inference.root_resolved_expr_ty(*expr) == ty
            }
            DeferredKind::IteratorItem { item, .. } => self.inference.root_resolved_ty(item) == ty,
            DeferredKind::Pattern { pat, .. } => {
                // A binding can be read before its tuple/record pattern has a shape to project.
                // Its eventual field type must remain free to become `!` as well.
                let mut pats = vec![*pat];
                while let Some(pat) = pats.pop() {
                    let Some(data) = self.body.pat(pat) else {
                        continue;
                    };
                    if let PatKind::Binding {
                        binding: Some(binding),
                        ..
                    } = data.kind
                        && self
                            .inference
                            .root_resolved_ty(&self.inference.binding_ty(binding))
                            == ty
                    {
                        return true;
                    }
                    pats.extend(data.kind.child_pats());
                }
                false
            }
            DeferredKind::Coerce { .. } => false,
        })
    }

    /// A supplied previous input keeps changes made during an attempt visible to the retry loop.
    /// Without one, queue against the current input.
    pub(super) fn defer(&mut self, kind: DeferredKind, previous_input: Option<Ty>) {
        if matches!(&kind, DeferredKind::Call { call, .. } if self.inference.call_is_complete(*call))
        {
            return;
        }
        let input = self.deferred_input(&kind);
        let needs_fulfillment = matches!(&kind, DeferredKind::Call { call, .. } if self.inference.call_needs_fulfillment(*call));
        let input_changed = previous_input
            .as_ref()
            .is_some_and(|previous| previous != &input);
        // Variables can gain evidence later. A failed operation on a fully known, unchanged
        // input has nothing left to wait for, unless the call's own solver inputs changed.
        if input.has_var() || input_changed || needs_fulfillment {
            self.deferred.push_back(Deferred {
                kind,
                // Calls also track obligation and projection inputs. A change there requests a
                // retry even if the outer call input stayed the same.
                input: if needs_fulfillment {
                    None
                } else {
                    Some(previous_input.unwrap_or(input))
                },
            });
        }
    }

    /// Try with the evidence available now. Keep the pre-attempt input if unfinished: the attempt
    /// itself may commit guidance that makes its next question different.
    pub(super) fn run_or_defer(&mut self, kind: DeferredKind) -> anyhow::Result<()> {
        let input = self.deferred_input(&kind);
        if !self
            .try_deferred(&kind)
            .context("attempt deferred inference")?
        {
            self.defer(kind, Some(input));
        }
        Ok(())
    }

    /// Retry changed questions until a full round has nothing to try. For example, learning the
    /// type of `source` can unlock `source.field`, whose result can unlock another pending lookup.
    /// Canonical inputs include variable identity and equality guidance, so progress need not mean
    /// that a type became concrete. Unchanged questions stay queued for a later checkpoint.
    pub(super) fn fulfill_pending(&mut self) -> anyhow::Result<()> {
        // Bound the retry work at one checkpoint. Reaching the limit leaves useful facts in
        // place for body completion to publish.
        for _ in 0..128 {
            rg_std::check_cancel!(self.context, "fulfill pending inference");
            let mut attempted = false;
            // Keep the other operations visible while retrying one: a coercion must know whether
            // its source still has an unfinished producer. New work waits for the next round.
            let pending_count = self.deferred.len();
            for _ in 0..pending_count {
                let operation = self
                    .deferred
                    .pop_front()
                    .expect("round retains its pending work");
                rg_std::check_cancel!(self.context, "retry pending inference");
                let input = self.deferred_input(&operation.kind);
                // Selecting a generic producer can make coercion safe without making its result
                // concrete. In that case the type key stays unchanged, but the wait is over.
                let coercion_ready = match &operation.kind {
                    DeferredKind::Coerce { expr, .. } => {
                        self.can_coerce_ty(&self.inference.expr_ty(*expr))
                    }
                    DeferredKind::BranchResult { branches, .. } => {
                        branches.iter().all(|ty| self.can_coerce_ty(ty))
                    }
                    _ => false,
                };
                if operation.input.as_ref() == Some(&input) && !coercion_ready {
                    self.deferred.push_back(operation);
                    continue;
                }
                attempted = true;
                crate::profile::metric::DEFERRED_RETRIES.inc();
                if !self
                    .try_deferred(&operation.kind)
                    .context("retry deferred inference")?
                {
                    self.defer(operation.kind, Some(input));
                }
            }
            if !attempted {
                return Ok(());
            }
        }
        self.inference_exhausted = true;
        crate::profile::metric::DEFERRED_EXHAUSTIONS.inc();
        Ok(())
    }

    /// Snapshot the types this operation depends on, expanding solved variables for comparison.
    /// Tuples here group dependencies into a key; they are not types of source expressions.
    fn deferred_input(&self, kind: &DeferredKind) -> Ty {
        let expr_ty = |expr| self.inference.expr_ty(expr);
        let input = match kind {
            DeferredKind::Call { call } => {
                let (receiver, args) = match &self.body.expr_unchecked(*call).kind {
                    ExprKind::Call { args, .. } => (None, args),
                    ExprKind::MethodCall { receiver, args, .. } => (*receiver, args),
                    _ => unreachable!("pending call owns a call expression"),
                };
                // Before selection, only receiver evidence can change call lookup. Once a
                // signature is selected, arguments, the result, and its generic bindings can
                // all change the remaining proof or projection work.
                if self.inference.selected_call_function(*call).is_none() {
                    receiver.map(expr_ty).unwrap_or(Ty::Unknown)
                } else {
                    Ty::tuple(
                        receiver
                            .into_iter()
                            .chain(args.iter().copied())
                            .chain([*call])
                            .map(expr_ty)
                            .chain([self.inference.call_input(*call)])
                            .collect(),
                    )
                }
            }
            DeferredKind::Member { expr } => match self.body.expr_unchecked(*expr).kind {
                ExprKind::Field { base, .. } | ExprKind::Index { base, .. } => {
                    base.map(expr_ty).unwrap_or(Ty::Unknown)
                }
                _ => unreachable!("pending member owns a field or index expression"),
            },
            DeferredKind::Pattern { expected, .. } => expected.clone(),
            DeferredKind::IteratorItem { iterable, .. } => expr_ty(*iterable),
            DeferredKind::TryOutput { expr } => match self.body.expr_unchecked(*expr).kind {
                ExprKind::Wrapper { inner, .. } => inner.map(expr_ty).unwrap_or(Ty::Unknown),
                _ => unreachable!("pending try owns a wrapper expression"),
            },
            DeferredKind::Operator { expr } => match self.body.expr_unchecked(*expr).kind {
                ExprKind::Unary { expr: inner, .. } => {
                    Ty::tuple(inner.into_iter().map(expr_ty).collect())
                }
                ExprKind::Binary { lhs, rhs, .. } => {
                    Ty::tuple(lhs.into_iter().chain(rhs).map(expr_ty).collect())
                }
                _ => unreachable!("pending operator owns a unary or binary expression"),
            },
            DeferredKind::Coerce { expr, expected } => {
                Ty::tuple(vec![expr_ty(*expr), expected.clone()])
            }
            DeferredKind::BranchResult { expr, branches } => {
                Ty::tuple(branches.iter().cloned().chain([expr_ty(*expr)]).collect())
            }
        };
        self.inference.table().canonicalize(&input)
    }

    /// Perform semantic work using already-visited expressions. A true result means no more work
    /// is needed here, including when syntax or declarations are unavailable; it is not a claim
    /// that the source type-checks. A false result lets the enqueue policy decide whether to wait.
    fn try_deferred(&mut self, kind: &DeferredKind) -> anyhow::Result<bool> {
        match kind {
            DeferredKind::Coerce { expr, expected } => Ok(self.try_coerce_expr_ty(*expr, expected)),
            DeferredKind::BranchResult { expr, branches } => {
                if branches.is_empty() {
                    // This is missing syntax, such as a match with no arms. An actual empty block
                    // is a branch expression with type `()`, handled by the expression walker.
                    self.inference.set_expr_ty(*expr, Ty::Unknown);
                    return Ok(true);
                }
                // In `if flag { user.abort() } else { 1 }`, the integer branch can supply the
                // result immediately. Leave the pending method result separate: discovering `!`
                // later must not turn that integer into a conflict.
                let result_ty = self.inference.expr_slot(*expr);
                let mut has_value_result = false;
                let mut complete = true;
                for branch in branches {
                    let branch = self.inference.root_resolved_ty(branch);
                    if matches!(branch, Ty::Never) {
                        continue;
                    }
                    has_value_result = true;
                    if !self.can_coerce_ty(&branch) {
                        complete = false;
                        continue;
                    }
                    // Root resolution keeps already-detected cycles as unknown, as in
                    // `value = match state { Keep => value, Change => next }`.
                    self.inference.constrain_infer_tys(&result_ty, &branch);
                }
                if !has_value_result {
                    self.inference.set_expr_ty(*expr, Ty::Never);
                }
                Ok(complete)
            }
            DeferredKind::Call { call } => {
                let body = self.body;
                let (receiver, args) = match &body.expr_unchecked(*call).kind {
                    ExprKind::Call { args, .. } => (None, args.as_slice()),
                    ExprKind::MethodCall { receiver, args, .. } => (*receiver, args.as_slice()),
                    _ => unreachable!("pending call owns a call expression"),
                };
                if let Some(transfer) = self
                    .prepare_call(*call, receiver)
                    .context("select pending call signature")?
                {
                    self.finish_call(transfer, args)
                        .context("complete pending call")?;
                }
                Ok(self.inference.call_is_complete(*call))
            }
            DeferredKind::Pattern { pat, expected } => self
                .try_infer_pat(*pat, expected)
                .context("project pending pattern"),
            DeferredKind::Member { expr } => {
                let base = match self.body.expr_unchecked(*expr).kind {
                    ExprKind::Field {
                        base: Some(base), ..
                    }
                    | ExprKind::Index {
                        base: Some(base), ..
                    } => base,
                    _ => return Ok(true),
                };
                crate::profile::metric::PROJECTION_ATTEMPTS.inc();
                let base_ty = self
                    .inference
                    .table()
                    .canonicalize(&self.inference.expr_ty(base));
                let ty = match &self.body.expr_unchecked(*expr).kind {
                    ExprKind::Field {
                        field: Some(field), ..
                    } => {
                        let targets = self
                            .context
                            .fields()
                            .resolve_for_ty(&base_ty, field)
                            .context("project field")?;
                        self.inference
                            .set_expr_resolution(*expr, targets.resolution());
                        targets.single_ty().cloned()
                    }
                    ExprKind::Index { .. } => {
                        let mut ty = &base_ty;
                        while let Ty::Reference { inner, .. } = ty {
                            ty = inner;
                        }
                        match ty {
                            Ty::Array { inner, .. } | Ty::Slice(inner) => {
                                Some(inner.as_ref().clone())
                            }
                            _ => None,
                        }
                    }
                    _ => return Ok(true),
                };
                if let Some(ty) = ty {
                    // A projected `?T` is already useful: linking it to the destination carries
                    // future evidence without another lookup. Unknowns and associated types can
                    // still need another projection after the base type changes.
                    self.inference.set_expr_infer_ty(*expr, ty.clone());
                    return Ok(!ty.has_unknown() && !ty.has_projection());
                }
                Ok(false)
            }
            DeferredKind::IteratorItem { iterable, item } => {
                // The lang item identifies `IntoIterator::into_iter`. Its trait owner supplies
                // the `Item` projection needed for a `for` loop's pattern.
                let Some(function) = self
                    .context
                    .item_lookup_query()
                    .lang_function(LangItem::IntoIter)
                else {
                    return Ok(true);
                };
                let Some(data) = self
                    .context
                    .item_query()
                    .function_data(function)
                    .context("resolve IntoIterator")?
                else {
                    return Ok(true);
                };
                let ItemOwner::Trait(trait_id) = data.owner else {
                    return Ok(true);
                };
                let ty = self.inference.root_resolved_expr_ty(*iterable);
                let goal = TraitGoal::new(
                    ty,
                    TraitDefRef::new(function.origin, trait_id),
                    GenericArgs::empty(),
                );
                let Some(projection) = self
                    .context
                    .trait_selection()
                    .normalize_assoc_type(&goal, "Item", self.inference.table())
                    .context("project iterator item")?
                else {
                    return Ok(false);
                };
                *self.inference.table_mut() = projection.table;
                self.inference.constrain_infer_tys(item, &projection.ty);
                Ok(!projection.ty.has_unknown() && !projection.ty.has_projection())
            }
            DeferredKind::TryOutput { expr } => {
                let ExprKind::Wrapper {
                    inner: Some(inner), ..
                } = self.body.expr_unchecked(*expr).kind
                else {
                    return Ok(true);
                };
                // Project the first payload of the recognized Result/Option shapes.
                // TODO: Replace this shallow rule with Try::Output when inference coverage expands.
                let inner_ty = self.inference.root_resolved_expr_ty(inner);
                let mut outputs = ExpectedUnique::new();
                let item_query = self.context.item_query();
                for nominal in inner_ty.as_adts() {
                    let Ok(Some(name)) = item_query.type_def_name(nominal.def) else {
                        continue;
                    };
                    if matches!(name, "Result" | "Option")
                        && let Some(output) =
                            nominal.args.iter().find_map(|arg| arg.as_ty().cloned())
                    {
                        outputs.push(output);
                    }
                }
                let ty = outputs.into_ty();
                self.inference.set_expr_infer_ty(*expr, ty.clone());
                Ok(!matches!(ty, Ty::Unknown))
            }
            DeferredKind::Operator { expr, .. } => {
                match self.body.expr_unchecked(*expr).kind {
                    ExprKind::Unary {
                        op: Some(ExprUnaryOp::Deref),
                        expr: Some(inner),
                    } => {
                        // Retain a dereference result only when candidate types agree.
                        let inner_ty = self.inference.root_resolved_expr_ty(inner);
                        let mut candidates = ExpectedUnique::new();
                        for candidate in self
                            .context
                            .autoderef()
                            .candidates(AutoderefMode::ExplicitDeref, &inner_ty)
                        {
                            candidates.push(candidate.context("project dereference")?.ty().clone());
                        }
                        self.inference
                            .set_expr_infer_ty(*expr, candidates.into_ty());
                    }
                    ExprKind::Unary {
                        op: Some(op),
                        expr: Some(inner),
                    } => {
                        self.inference.set_expr_unary_from_inner(*expr, op, inner);
                    }
                    ExprKind::Binary {
                        lhs: Some(lhs),
                        rhs: Some(rhs),
                        op: Some(op),
                    } => {
                        self.inference
                            .set_expr_binary_from_operands(*expr, op, lhs, rhs);
                    }
                    _ => return Ok(true),
                }
                Ok(!matches!(
                    self.inference.root_resolved_expr_ty(*expr),
                    Ty::Unknown
                        | Ty::InferVar {
                            kind: rg_ty::inference::InferVarKind::Type,
                            ..
                        }
                ))
            }
        }
    }
}
