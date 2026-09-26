//! Pending semantic operations have live inputs and a stable destination. Syntax is never queued:
//! retries only perform the lookup, projection, obligation, or coercion that could not finish earlier.
//!
//! There are two kinds of waiting. Looking up `value.field` needs enough receiver shape to find
//! the field; that work stays here. Once we can express a question such as `Iterator::Item = ?T`,
//! the inference table's goal queue can solve it. Each checkpoint runs the solver first, then
//! retries body operations whose inputs gained evidence.

use anyhow::Context as _;
use rg_def_map::DefMapSource;
use rg_ir_model::{ExprId, ItemOwner, Mutability, PatId, TraitDefRef};
use rg_item_tree::LangItem;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::ExpectedUnique;
use rg_ty::solver::{Ty, TyShape};

use super::{BodyInference, InferenceState};
use crate::{
    ExprUnaryOp,
    body::{ExprKind, PatKind},
};

// Limit solver/body alternations at one checkpoint. The solver separately limits passes over
// its pending goals; each of these rounds can include several of those passes.
const MAX_DEFERRED_INFERENCE_ROUNDS: usize = 128;

/// One unfinished operation and the input against which it last ran or was queued.
/// The operation reads live slots; the stored input is only a snapshot for deciding when to retry.
pub(super) struct Deferred<'s> {
    kind: DeferredKind<'s>,
    // `None` requests an attempt at the next checkpoint, even without an outer input change.
    input: Option<Ty<'s>>,
}

pub(super) enum DeferredKind<'s> {
    Call {
        call: ExprId,
    },
    Member {
        expr: ExprId,
    },
    Pattern {
        pat: PatId,
        expected: Ty<'s>,
        default_ref: Option<Mutability>,
    },
    IteratorItem {
        iterable: ExprId,
        item: Ty<'s>,
    },
    TryOutput {
        expr: ExprId,
    },
    Operator {
        expr: ExprId,
    },
    Coerce {
        expr: ExprId,
        expected: Ty<'s>,
    },
    BranchResult {
        expr: ExprId,
        branches: Vec<Ty<'s>>,
    },
}

impl<'s, 'query, D, I> BodyInference<'s, 'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    /// Apply an expression expectation without equating a possible `!` to the expected type.
    /// Other supported cases use ordinary unification, but an unfinished producer needs to
    /// determine its result before we can choose between these two cases.
    pub(super) fn coerce_expr_ty(&mut self, expr: ExprId, expected: &Ty<'s>) {
        if !self.try_coerce_expr_ty(expr, expected) {
            self.defer(
                DeferredKind::Coerce {
                    expr,
                    expected: *expected,
                },
                None,
            );
        }
    }

    /// Publish useful expectations even when a producer could not be resolved. For example,
    /// `let size = number.try_into().ok()?; Some(size)` can learn `size` from the return type
    /// despite incomplete conversion lookup. Only do this after semantic work has stopped:
    /// until then, a pending producer may still turn out to return `!`.
    /// Consume the context so these fallback equalities cannot feed another lookup attempt.
    pub(super) fn finish_coercions(mut self) -> InferenceState<'s> {
        for operation in self.deferred {
            match operation.kind {
                DeferredKind::Coerce { expr, expected } => {
                    self.inference.constrain_expr_ty(expr, &expected);
                }
                DeferredKind::BranchResult { expr, branches } => {
                    let result = self.inference.expr_slot(expr);
                    for branch in branches {
                        let branch = self.inference.root_resolved_ty(&branch);
                        if !matches!((branch).shape(), TyShape::Never) {
                            self.inference.constrain_infer_tys(&result, &branch);
                        }
                    }
                }
                _ => {}
            }
        }
        self.inference
    }

    fn try_coerce_expr_ty(&mut self, expr: ExprId, expected: &Ty<'s>) -> bool {
        if matches!((expected).shape(), TyShape::Unknown) {
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
    fn can_coerce_ty(&self, ty: &Ty<'s>) -> bool {
        let ty = self.inference.root_resolved_ty(ty);
        if self.inference.table().has_pending_projection(ty) {
            return false;
        }
        if !matches!(
            (ty).shape(),
            TyShape::InferVar {
                kind: rg_ty::solver::InferVarKind::Type,
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
    pub(super) fn defer(&mut self, kind: DeferredKind<'s>, previous_input: Option<Ty<'s>>) {
        if matches!(&kind, DeferredKind::Call { call, .. } if self.inference.call_is_selected(*call))
        {
            return;
        }
        let input = self.deferred_input(&kind);
        let input_changed = previous_input
            .as_ref()
            .is_some_and(|previous| previous != &input);
        // Variables can gain evidence later. A failed operation on a fully known, unchanged
        // input has nothing left to wait for. Solver obligations have their own shared queue.
        if input.has_var() || input_changed {
            self.deferred.push_back(Deferred {
                kind,
                input: Some(previous_input.unwrap_or(input)),
            });
        }
    }

    /// Try with the evidence available now. Keep the pre-attempt input if unfinished: the attempt
    /// itself may commit guidance that makes its next question different.
    pub(super) fn run_or_defer(&mut self, kind: DeferredKind<'s>) -> anyhow::Result<()> {
        let input = self.deferred_input(&kind);
        if !self
            .try_deferred(&kind)
            .context("attempt deferred inference")?
        {
            self.defer(kind, Some(input));
        }
        Ok(())
    }

    /// Let trait goals and deferred body operations pass new type information to each other.
    ///
    /// For example, solving a trait goal can reveal the type of `source`, letting us look up
    /// `source.field`. If the field's type is `T::Item`, relating it to the expression slot can
    /// add another goal. Solving that goal can then unlock `source.field.method()`.
    ///
    /// A round first runs the solver until its goals stop learning from each other, then retries
    /// body operations with changed inputs. The solver cannot perform those body operations, and
    /// they can add goals after it returns, so both levels need their own retry loop.
    ///
    /// Stop when a full round has nothing to try. Input comparisons include variable identity
    /// and equality guidance, so progress need not mean that a type became concrete. Unchanged
    /// questions stay queued for a later checkpoint.
    pub(super) fn fulfill_pending(&mut self) -> anyhow::Result<()> {
        for _ in 0..MAX_DEFERRED_INFERENCE_ROUNDS {
            rg_std::check_cancel!(self.context, "fulfill pending inference");
            // Settle the queued trait goals before checking which body inputs have changed.
            let _ = self.inference.table().fulfill();
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
            // An attempt can add solver goals or help an operation we already passed in this
            // scan. Allow another round even if it learned nothing; unchanged inputs are skipped.
            if !attempted {
                return Ok(());
            }
        }
        // Report exhaustion while keeping the facts learned so far for body completion to publish.
        self.inference_exhausted = true;
        crate::profile::metric::DEFERRED_EXHAUSTIONS.inc();
        Ok(())
    }

    /// Snapshot the types this operation depends on, expanding solved variables for comparison.
    /// Tuples here group dependencies into a key; they are not types of source expressions.
    fn deferred_input(&self, kind: &DeferredKind<'s>) -> Ty<'s> {
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
                    receiver.map(expr_ty).unwrap_or(self.cx.unknown())
                } else {
                    self.cx.tuple(
                        receiver
                            .into_iter()
                            .chain(args.iter().copied())
                            .chain([*call])
                            .map(expr_ty)
                            .chain([self.inference.call_input(*call)])
                            .collect::<Vec<_>>(),
                    )
                }
            }
            DeferredKind::Member { expr } => match self.body.expr_unchecked(*expr).kind {
                ExprKind::Field { base, .. } | ExprKind::Index { base, .. } => {
                    base.map(expr_ty).unwrap_or(self.cx.unknown())
                }
                _ => unreachable!("pending member owns a field or index expression"),
            },
            DeferredKind::Pattern { expected, .. } => *expected,
            DeferredKind::IteratorItem { iterable, .. } => expr_ty(*iterable),
            DeferredKind::TryOutput { expr } => match self.body.expr_unchecked(*expr).kind {
                ExprKind::Wrapper { inner, .. } => inner.map(expr_ty).unwrap_or(self.cx.unknown()),
                _ => unreachable!("pending try owns a wrapper expression"),
            },
            DeferredKind::Operator { expr } => match self.body.expr_unchecked(*expr).kind {
                ExprKind::Unary { expr: inner, .. } => self
                    .cx
                    .tuple(inner.into_iter().map(expr_ty).collect::<Vec<_>>()),
                ExprKind::Binary { lhs, rhs, .. } => self
                    .cx
                    .tuple(lhs.into_iter().chain(rhs).map(expr_ty).collect::<Vec<_>>()),
                _ => unreachable!("pending operator owns a unary or binary expression"),
            },
            DeferredKind::Coerce { expr, expected } => {
                self.cx.tuple(vec![expr_ty(*expr), *expected])
            }
            DeferredKind::BranchResult { expr, branches } => self.cx.tuple(
                branches
                    .iter()
                    .cloned()
                    .chain([expr_ty(*expr)])
                    .collect::<Vec<_>>(),
            ),
        };
        self.inference.table().canonicalize(&input)
    }

    /// Perform semantic work using already-visited expressions. A true result means no more work
    /// is needed here, including when syntax or declarations are unavailable; it is not a claim
    /// that the source type-checks. A false result lets the enqueue policy decide whether to wait.
    fn try_deferred(&mut self, kind: &DeferredKind<'s>) -> anyhow::Result<bool> {
        match kind {
            DeferredKind::Coerce { expr, expected } => Ok(self.try_coerce_expr_ty(*expr, expected)),
            DeferredKind::BranchResult { expr, branches } => {
                if branches.is_empty() {
                    // This is missing syntax, such as a match with no arms. An actual empty block
                    // is a branch expression with type `()`, handled by the expression walker.
                    self.inference.set_expr_ty(*expr, self.cx.unknown());
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
                    if matches!((branch).shape(), TyShape::Never) {
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
                    self.inference.set_expr_ty(*expr, self.cx.never());
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
                Ok(self.inference.call_is_selected(*call))
            }
            DeferredKind::Pattern {
                pat,
                expected,
                default_ref,
            } => self
                .try_infer_pat(*pat, expected, *default_ref)
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
                        let target = self
                            .context
                            .live()
                            .field(base_ty, field, self.inference.table())
                            .context("project field")?;
                        target.map(|(resolution, ty)| {
                            self.inference.set_expr_resolution(*expr, resolution);
                            ty
                        })
                    }
                    ExprKind::Index { .. } => {
                        let mut ty = base_ty;
                        while let TyShape::Reference { inner, .. } = ty.shape() {
                            ty = inner;
                        }
                        match ty.shape() {
                            TyShape::Array { inner, .. } | TyShape::Slice(inner) => Some(inner),
                            _ => None,
                        }
                    }
                    _ => return Ok(true),
                };
                if let Some(ty) = ty {
                    // A projected `?T` is already useful: linking it to the destination carries
                    // future evidence without another lookup. Unknowns and associated types can
                    // still need another projection after the base type changes.
                    self.inference.set_expr_ty(*expr, ty);
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
                let trait_ref = TraitDefRef::new(function.origin, trait_id);
                let Some(projection) = self
                    .context
                    .live()
                    .projection(ty, trait_ref, "Item", self.inference.table())
                    .context("project iterator item")?
                else {
                    return Ok(false);
                };
                self.inference.constrain_infer_tys(item, &projection);
                // Fulfillment now owns the equality, including any later changes to the iterable.
                Ok(true)
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
                if let Some(nominal) = inner_ty.as_adt()
                    && let Some(name) = item_query
                        .type_def_name(nominal.def)
                        .context("resolve try operand type")?
                    && matches!(name, "Result" | "Option")
                    && let Some(output) = nominal.args.iter().find_map(|arg| arg.as_ty())
                {
                    outputs.push(output);
                }
                let ty = outputs.into_option().unwrap_or(self.cx.unknown());
                self.inference.set_expr_ty(*expr, ty);
                Ok(!matches!((ty).shape(), TyShape::Unknown))
            }
            DeferredKind::Operator { expr, .. } => {
                match self.body.expr_unchecked(*expr).kind {
                    ExprKind::Unary {
                        op: Some(ExprUnaryOp::Deref),
                        expr: Some(inner),
                    } => {
                        // An explicit `*value` takes one dereference step after the original type.
                        let inner_ty = self.inference.root_resolved_expr_ty(inner);
                        let ty = self
                            .inference
                            .table()
                            .autoderef(inner_ty)
                            .nth(1)
                            .unwrap_or(self.cx.unknown());
                        self.inference.set_expr_ty(*expr, ty);
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
                    (self.inference.root_resolved_expr_ty(*expr)).shape(),
                    TyShape::Unknown
                        | TyShape::InferVar {
                            kind: rg_ty::solver::InferVarKind::Type,
                            ..
                        }
                ))
            }
        }
    }
}
