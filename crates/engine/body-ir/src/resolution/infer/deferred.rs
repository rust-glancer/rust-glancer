//! Pending semantic operations have live inputs and a stable destination. Syntax is never queued:
//! retries only perform the lookup, projection, obligation, or coercion that could not finish earlier.
//!
//! There are two kinds of waiting. Looking up `value.field` needs enough receiver shape to find
//! the field; that work stays here. Once we can express a question such as `Iterator::Item = ?T`,
//! the inference table's goal queue can solve it. Each checkpoint runs the solver first, then
//! retries body operations whose inputs gained evidence.

use anyhow::Context as _;
use rg_def_map::DefMapSource;
use rg_ir_model::{ExprId, ItemOwner, Mutability, PatId, ScopeId, TraitDefRef};
use rg_item_tree::LangItem;
use rg_semantic_ir::SemanticItemSource;
use rg_std::ExpectedUnique;
use rg_ty::{
    FnDefRef,
    solver::{AliasTy, DynTy, FnDefTy, List, TraitApplication, Ty, TyShape},
};

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
    Field {
        expr: ExprId,
    },
    Index {
        expr: ExprId,
    },
    Pattern {
        pat: PatId,
        scope: ScopeId,
        expected: Ty<'s>,
        default_ref: Option<Mutability>,
    },
    IteratorItem {
        iterable: ExprId,
        item: Ty<'s>,
    },
    Operator {
        expr: ExprId,
    },
    Coerce {
        expr: ExprId,
        expected: Ty<'s>,
    },
    BranchResult {
        result: Ty<'s>,
        branches: Vec<Ty<'s>>,
        never_fallback: bool,
    },
    BlockTail {
        result: Ty<'s>,
        statements: Vec<Ty<'s>>,
        tail: Ty<'s>,
    },
}

impl<'s, 'query, D, I> BodyInference<'s, 'query, D, I>
where
    D: DefMapSource + Copy,
    I: SemanticItemSource<'query> + Copy,
{
    /// Apply an expression expectation, allowing divergence and coercions.
    /// An unfinished producer needs to supply its type before we can choose the relation.
    pub(super) fn coerce_expr_ty(&mut self, expr: ExprId, expected: &Ty<'s>) {
        let mut expected = self.inference.root_resolved_ty(expected);
        if matches!(expected.shape(), TyShape::Alias(AliasTy::Projection(_))) {
            // `F::Factory` can reveal a function pointer after an earlier argument determines F.
            // Keep the normalized slot so retries can use its shape before relating the item.
            expected = self.inference.table().normalize(expected);
        }
        if !self.try_coerce_expr_ty(expr, &expected) {
            self.defer(DeferredKind::Coerce { expr, expected }, None);
        }
    }

    /// Keep the tail separate from breaks and returns. A statement that later resolves to
    /// `!` removes the fallthrough route, while those other exits keep their values.
    pub(super) fn block_tail_slot(&mut self, statements: Vec<Ty<'s>>, tail: Ty<'s>) -> Ty<'s> {
        match self.block_fallthrough(&statements, tail) {
            Some(ty) => ty,
            None => {
                let result = self.inference.table_mut().new_type_var();
                self.defer(
                    DeferredKind::BlockTail {
                        result,
                        statements,
                        tail,
                    },
                    None,
                );
                result
            }
        }
    }

    pub(super) fn infer_async_output(&mut self, fallthrough: Ty<'s>) -> anyhow::Result<()> {
        let mut branches = self
            .return_target
            .values
            .take()
            .expect("async expression collects explicit returns");
        branches.push(fallthrough);
        self.run_or_defer(DeferredKind::BranchResult {
            result: self.return_target.expected,
            branches,
            never_fallback: true,
        })
    }

    fn block_fallthrough(&self, statements: &[Ty<'s>], tail: Ty<'s>) -> Option<Ty<'s>> {
        // Any diverging statement prevents reaching the tail, including a `let` initializer.
        // Check known divergence first; an earlier unresolved call cannot restore fallthrough.
        if statements
            .iter()
            .any(|ty| self.inference.root_resolved_ty(ty).is_never())
        {
            Some(self.cx.never())
        } else if statements.iter().all(|ty| self.can_coerce_ty(ty)) {
            Some(tail)
        } else {
            None
        }
    }

    /// Publish useful expectations even when a producer could not be resolved. For example,
    /// `let size = number.try_into().ok()?; Some(size)` can learn `size` from the return type
    /// despite incomplete conversion lookup. Only do this after semantic work has stopped:
    /// until then, a pending producer may still turn out to return `!`.
    /// Consume the context so these fallback equalities cannot feed another lookup attempt.
    pub(super) fn finish_coercions(mut self) -> InferenceState<'s> {
        let deferred = std::mem::take(&mut self.deferred);
        // Unresolved statement producers cannot prove divergence. Give their blocks the tail
        // fallback before merging branch results, so a synthetic slot is never mistaken for a
        // value branch merely because the block's control flow was still pending.
        for operation in &deferred {
            if let DeferredKind::BlockTail {
                result,
                statements,
                tail,
            } = &operation.kind
            {
                let tail = self.block_fallthrough(statements, *tail).unwrap_or(*tail);
                self.inference.constrain_infer_tys(result, &tail);
            }
        }
        for operation in deferred {
            match operation.kind {
                DeferredKind::Coerce { expr, expected } => {
                    self.inference.constrain_expr_ty(expr, &expected);
                }
                DeferredKind::BranchResult {
                    result, branches, ..
                } => {
                    if self.try_join_constructors(result, &branches) {
                        continue;
                    }
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
        let expected = self.inference.root_resolved_ty(expected);
        if expected.is_unknown() {
            return true;
        }
        let source = self.inference.root_resolved_expr_ty(expr);
        if !self.coercion_ready(source, expected) {
            return false;
        }

        if matches!(expected.shape(), TyShape::FnPointer { .. })
            && let TyShape::FnDef(item) = source.shape()
            && let Some(pointer) = item.constructor_pointer(self.cx)
            && self.inference.table().try_unify(pointer, expected).is_ok()
        {
            self.inference.set_coerced_expr_ty(expr, expected);
            return true;
        }

        if let Some(object) = self.trait_object_pointee(expected) {
            let source_inner = match (source.shape(), expected.shape()) {
                (
                    TyShape::Reference {
                        inner, mutability, ..
                    },
                    TyShape::Reference {
                        mutability: target_mutability,
                        ..
                    },
                ) if mutability == target_mutability || target_mutability == Mutability::Shared => {
                    Some(inner)
                }
                (TyShape::Adt(source), TyShape::Adt(target)) if source.def == target.def => {
                    source.args.first().and_then(|arg| arg.as_ty())
                }
                _ => None,
            };
            if let Some(source_inner) = source_inner
                && !matches!(
                    self.inference.root_resolved_ty(&source_inner).shape(),
                    TyShape::Dyn(_)
                )
            {
                // &Iter<?T> -> &dyn Iterator<Item = u8> keeps Iter's identity. The ordinary
                // trait and associated-type goals can still learn ?T = u8, including after
                // an unfinished call supplies more information about the concrete iterator.
                for clause in object.clauses(self.cx, source_inner) {
                    self.inference.table().register(clause);
                }
                self.inference.set_coerced_expr_ty(expr, expected);
                return true;
            }
        }
        if self.inference.constrain_expr_ty(expr, &expected) {
            return true;
        }
        if self
            .inference
            .table()
            .try_coerce_reference(source, expected)
        {
            self.inference.set_coerced_expr_ty(expr, expected);
            return true;
        }
        // A later use can determine the arguments of a Deref impl and reveal its target.
        // Leave conflicting references eligible for a retry when either type changes.
        source.reference_inner().is_none() || expected.reference_inner().is_none()
    }

    fn coercion_ready(&self, source: Ty<'s>, expected: Ty<'s>) -> bool {
        let mut producer = source;
        if expected.reference_inner().is_some() {
            // In `first(&items?)`, the projection may yield Vec<T>. Wait before equating its
            // destination with the slice, so the borrow can use Deref when the type is known.
            while let Some((inner, _)) = producer.reference_inner() {
                producer = self.inference.root_resolved_ty(&inner);
            }
        }
        self.can_coerce_ty(&producer)
            && !self.constructor_waits_for_projection(source, expected)
            // In `let object: Box<dyn Trait> = id(Box::new(value))`, wait for the argument
            // to supply Box<Concrete> before applying the conversion to the result.
            && !(source.is_var() && self.trait_object_pointee(expected).is_some())
    }

    fn constructor_waits_for_projection(&self, source: Ty<'s>, expected: Ty<'s>) -> bool {
        matches!(
            source.shape(),
            TyShape::FnDef(FnDefTy {
                def: FnDefRef::Constructor(_),
                ..
            })
        ) && self.inference.table().has_pending_projection(expected)
    }

    /// Distinct items can share a pointer type, as in `if flag { Event::A } else { Event::B }`.
    /// Relate their signatures in one trial so a failed join cannot refine an earlier branch.
    fn try_join_constructors(&self, result: Ty<'s>, branches: &[Ty<'s>]) -> bool {
        let table = self.inference.table();
        let result = table.resolve_root_var(result);
        let mut pointer = matches!(result.shape(), TyShape::FnPointer { .. }).then_some(result);
        let mut items = ExpectedUnique::new();
        for &branch in branches {
            let branch = table.resolve_root_var(branch);
            match branch.shape() {
                TyShape::FnDef(item) => items.push(item.def),
                TyShape::FnPointer { .. } => {
                    pointer.get_or_insert(branch);
                }
                TyShape::Never | TyShape::Unknown | TyShape::InferVar { .. } => {}
                _ => return false,
            }
        }
        if items.is_empty() || (pointer.is_none() && !items.is_ambiguous()) {
            return false;
        }

        table
            .commit_if_some(|table| -> Result<_, std::convert::Infallible> {
                if pointer.is_none() {
                    pointer = branches.iter().find_map(|&branch| {
                        match table.resolve_root_var(branch).shape() {
                            TyShape::FnDef(item) => item.constructor_pointer(self.cx),
                            _ => None,
                        }
                    });
                }
                let Some(pointer) = pointer else {
                    return Ok(None);
                };
                if table.try_unify(result, pointer).is_err() {
                    return Ok(None);
                }
                for &branch in branches {
                    let branch = table.resolve_root_var(branch);
                    let input = match branch.shape() {
                        TyShape::Never => continue,
                        TyShape::FnDef(item) => {
                            let Some(input) = item.constructor_pointer(self.cx) else {
                                return Ok(None);
                            };
                            input
                        }
                        _ => branch,
                    };
                    if table.try_unify(input, pointer).is_err() {
                        return Ok(None);
                    }
                }
                Ok(Some(()))
            })
            .expect("function pointer join is infallible")
            .is_some()
    }

    /// Find the trait object inside a conversion target such as `&dyn Trait` or `Arc<dyn Trait>`.
    ///
    /// We recognize references, Box, Arc, and Rc directly so these common conversions don't need
    /// general Unsize/CoerceUnsized solving. This shortcut keeps inference simpler while covering
    /// everyday uses of trait objects. The object's bounds still constrain the concrete pointee.
    /// Compiler identities keep renamed imports working without treating arbitrary containers
    /// as pointers that support this conversion.
    ///
    /// TODO: Follow coercion implementations when a user needs wrappers outside this short list.
    fn trait_object_pointee(&self, ty: Ty<'s>) -> Option<DynTy<'s>> {
        let inner = match self.inference.root_resolved_ty(&ty).shape() {
            TyShape::Reference { inner, .. } => inner,
            TyShape::Adt(adt)
                if [LangItem::OwnedBox, LangItem::Arc, LangItem::Rc]
                    .into_iter()
                    .any(|item| {
                        self.context.item_lookup_query().lang_type(item) == Some(adt.def)
                    }) =>
            {
                adt.args.first()?.as_ty()?
            }
            _ => return None,
        };
        match self.inference.root_resolved_ty(&inner).shape() {
            TyShape::Dyn(object) => Some(object),
            _ => None,
        }
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
            DeferredKind::Call { call: expr }
            | DeferredKind::Field { expr }
            | DeferredKind::Index { expr }
            | DeferredKind::Operator { expr } => self.inference.root_resolved_expr_ty(*expr) == ty,
            DeferredKind::BranchResult { result, .. } | DeferredKind::BlockTail { result, .. } => {
                self.inference.root_resolved_ty(result) == ty
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
                let coercion_ready = match &operation.kind {
                    DeferredKind::BranchResult { branches, .. } => {
                        branches.iter().all(|ty| self.can_coerce_ty(ty))
                    }
                    DeferredKind::BlockTail {
                        statements, tail, ..
                    } => self.block_fallthrough(statements, *tail).is_some(),
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
    pub(super) fn deferred_input(&self, kind: &DeferredKind<'s>) -> Ty<'s> {
        let expr_ty = |expr| self.inference.expr_ty(expr);
        let input = match kind {
            DeferredKind::Call { call } => match &self.body.expr_unchecked(*call).kind {
                // Multiple callable bounds of the same arity can need argument evidence.
                ExprKind::Call {
                    callee: Some(callee),
                    args,
                } => self.cx.tuple(
                    std::iter::once(*callee)
                        .chain(args.iter().copied())
                        .map(expr_ty)
                        .collect::<Vec<_>>(),
                ),
                ExprKind::MethodCall { receiver, .. } => {
                    receiver.map(expr_ty).unwrap_or(self.cx.unknown())
                }
                ExprKind::Call { callee: None, .. } => self.cx.unknown(),
                _ => unreachable!("pending call owns a call expression"),
            },
            DeferredKind::Field { expr } => match self.body.expr_unchecked(*expr).kind {
                ExprKind::Field { base, .. } => base.map(expr_ty).unwrap_or(self.cx.unknown()),
                _ => unreachable!("pending field owns a field expression"),
            },
            DeferredKind::Index { expr } => match self.body.expr_unchecked(*expr).kind {
                ExprKind::Index { base, index } => self.cx.tuple([
                    base.map(expr_ty).unwrap_or(self.cx.unknown()),
                    index.map(expr_ty).unwrap_or(self.cx.unknown()),
                ]),
                _ => unreachable!("pending index owns an index expression"),
            },
            DeferredKind::Pattern { expected, .. } => *expected,
            DeferredKind::IteratorItem { iterable, .. } => expr_ty(*iterable),
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
                let source = self.inference.root_resolved_expr_ty(*expr);
                let target = self.inference.root_resolved_ty(expected);
                // Selecting a generic producer can make coercion safe while its result is
                // still the same variable. Include readiness so this change triggers a retry.
                let readiness = if self.coercion_ready(source, target) {
                    self.cx.unit()
                } else {
                    self.cx.never()
                };
                self.cx.tuple([source, target, readiness])
            }
            DeferredKind::BranchResult {
                result, branches, ..
            } => self.cx.tuple(
                branches
                    .iter()
                    .cloned()
                    .chain([*result])
                    .collect::<Vec<_>>(),
            ),
            DeferredKind::BlockTail {
                result,
                statements,
                tail,
            } => self.cx.tuple(
                statements
                    .iter()
                    .copied()
                    .chain([*tail, *result])
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
            DeferredKind::BlockTail {
                result,
                statements,
                tail,
            } => {
                let Some(tail) = self.block_fallthrough(statements, *tail) else {
                    return Ok(false);
                };
                self.inference.constrain_infer_tys(result, &tail);
                Ok(true)
            }
            DeferredKind::BranchResult {
                result,
                branches,
                never_fallback,
            } => {
                if branches.is_empty() {
                    // This is missing syntax, such as a match with no arms. An actual empty block
                    // is a branch expression with type `()`, handled by the expression walker.
                    return Ok(true);
                }

                // A pending branch can supply another item, changing the join to a pointer.
                // Wait before committing the result to the first known callable's identity.
                let callable = branches.iter().any(|ty| {
                    matches!(
                        self.inference.root_resolved_ty(ty).shape(),
                        TyShape::FnDef(_) | TyShape::FnPointer { .. }
                    )
                });
                if callable {
                    if branches.iter().any(|ty| !self.can_coerce_ty(ty)) {
                        return Ok(false);
                    }
                    if self.try_join_constructors(*result, branches) {
                        return Ok(true);
                    }
                }

                // In `if flag { user.abort() } else { 1 }`, the integer branch can supply the
                // result immediately. Leave the pending method result separate: discovering `!`
                // later must not turn that integer into a conflict.
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
                    self.inference.constrain_infer_tys(result, &branch);
                }
                if !has_value_result {
                    if *never_fallback {
                        self.diverging_outputs.push(*result);
                    } else {
                        self.inference.constrain_infer_tys(result, &self.cx.never());
                    }
                }
                Ok(complete)
            }
            DeferredKind::Call { call } => {
                let body = self.body;
                let args = match &body.expr_unchecked(*call).kind {
                    ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => args,
                    _ => unreachable!("pending call owns a call expression"),
                };
                let Some(prepared) = self
                    .prepare_call(*call)
                    .context("select pending call signature")?
                else {
                    return Ok(false);
                };
                self.finish_call(*call, prepared, args);
                Ok(true)
            }
            DeferredKind::Pattern {
                pat,
                scope,
                expected,
                default_ref,
            } => self
                .try_infer_pat(*pat, *scope, expected, *default_ref)
                .context("project pending pattern"),
            DeferredKind::Field { expr } => {
                let ExprKind::Field {
                    base: Some(base),
                    field: Some(ref field),
                    ..
                } = self.body.expr_unchecked(*expr).kind
                else {
                    return Ok(true);
                };
                crate::profile::metric::PROJECTION_ATTEMPTS.inc();
                let base_ty = self
                    .inference
                    .table()
                    .canonicalize(&self.inference.expr_ty(base));
                let target = self
                    .context
                    .live()
                    .field(base_ty, field, self.inference.table())
                    .context("project field")?;
                if let Some((resolution, ty)) = target {
                    // A projected `?T` is already useful: linking it to the destination carries
                    // future evidence without another lookup. Unknowns and associated types can
                    // still need another projection after the base type changes.
                    self.inference.set_expr_facts(*expr, resolution, ty);
                    return Ok(!ty.has_unknown() && !ty.has_projection());
                }
                Ok(false)
            }
            DeferredKind::Index { expr } => {
                let ExprKind::Index {
                    base: Some(base),
                    index: Some(index),
                } = self.body.expr_unchecked(*expr).kind
                else {
                    return Ok(true);
                };
                crate::profile::metric::PROJECTION_ATTEMPTS.inc();
                let base = self.inference.expr_slot(base);
                let index = self.inference.expr_slot(index);
                let Some(target) = self
                    .context
                    .live()
                    .index(base, index, self.inference.table())
                    .context("select indexing receiver")?
                else {
                    return Ok(false);
                };
                self.inference.table.adopt(target.table);
                self.inference.set_expr_ty(*expr, target.output);
                // The receiver is established. Fulfillment owns the projection from here,
                // including later changes to the index and output's generic arguments.
                // TODO: Validate mutable places with IndexMut when that pass is available.
                Ok(true)
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
                let trait_ref = TraitDefRef::new(function.store.origin, trait_id);
                let Some(projection) = self
                    .context
                    .live()
                    .projection(
                        TraitApplication {
                            def: trait_ref,
                            args: List::new(self.cx, &[ty.into()]),
                        },
                        "Item",
                        self.inference.table(),
                    )
                    .context("project iterator item")?
                else {
                    return Ok(false);
                };
                self.inference.constrain_infer_tys(item, &projection);
                // Fulfillment now owns the equality, including any later changes to the iterable.
                Ok(true)
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
