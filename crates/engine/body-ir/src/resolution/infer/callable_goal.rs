//! Callable trait goals that can be answered from body-local closure witnesses.
//!
//! This is the first bridge between the real closure type witness and trait obligations. It does
//! not try to prove capture semantics. For now, a closure witness can provide evidence for any of
//! `Fn`, `FnMut`, or `FnOnce` when the goal's argument shape fits the closure's written params.

use rg_ir_model::ExprId;
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_ty::{
    TraitGoal,
    inference::{InferGenericArg, InferTy},
};

use crate::{ir::ExprKind, resolution::BodyResolutionContext};

use super::{BodyInferenceCtx, BodyPatternInference};

/// Applies callable trait goals directly to body-local closure witnesses.
///
/// This is a "plug" for the trait solver that adds knowledge about Fn* trait without having to properly
/// model all the related machinery. For example, if we see that it's a closure and can link it to Fn* trait
/// anywhere, we directly apply this knowledge during trait solving.
///
/// This way we can "pretend" that it's a part of trait solving (which it will eventually be) so it's not
/// misplaced, but we avoid 80% of complexity at 20% of effort.
pub(crate) struct BodyCallableGoalSolver<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyCallableGoalSolver<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(super) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Use a goal like `Closure#n: FnOnce(User) -> R` as closure-local inference evidence.
    ///
    /// The goal is consumed only when it is really a callable trait goal on a closure witness.
    /// Unsupported or malformed goals return `false` so ordinary trait selection may still try to
    /// handle them. A callable arity mismatch is consumed but produces no evidence: we know this is
    /// the closure-callable approximation, but the written function shape does not fit this
    /// closure expression.
    pub(super) fn solve_goal(
        &self,
        inference: &mut BodyInferenceCtx,
        goal: &TraitGoal,
    ) -> Result<bool, PackageStoreError> {
        let Some((params, ret)) = self.callable_goal_args(goal)? else {
            return Ok(false);
        };
        self.solve_fn_trait_goal(inference, &goal.self_ty, params, ret)
    }

    /// Use already-projected callable fn-trait args as closure-local inference evidence.
    pub(super) fn solve_fn_trait_goal(
        &self,
        inference: &mut BodyInferenceCtx,
        self_ty: &InferTy,
        params: &[InferTy],
        ret: &InferTy,
    ) -> Result<bool, PackageStoreError> {
        let Some(closure) = Self::closure_expr(inference, self_ty) else {
            return Ok(false);
        };
        let Some(closure_data) = self.context.body().expr(closure).cloned() else {
            return Ok(false);
        };
        let ExprKind::Closure {
            params: closure_params,
            body,
            ..
        } = closure_data.kind
        else {
            return Ok(false);
        };
        if closure_params.len() != params.len() {
            return Ok(true);
        }

        // Function-call syntax gives parameter evidence positionally, so `FnOnce(User)` links the
        // first closure pattern to `User`, including destructuring such as
        // `FnOnce((Left, Right))` -> `|(left, right)|`.
        let pattern_inference = BodyPatternInference::new(self.context);
        for (closure_param, expected_ty) in closure_params.iter().zip(params.iter()) {
            let Some(pat) = closure_param.pat else {
                continue;
            };
            let expected_ty = inference.root_resolved_ty(expected_ty);
            if expected_ty.has_unknown_or_syntax() {
                continue;
            }
            let _ = pattern_inference.link_pat(inference, pat, &expected_ty);
        }

        // Return evidence flows in the opposite direction too: if `ret` is `?R` and the closure
        // body is known to be `Name`, this solves `?R = Name` for the caller that owns the goal.
        if let Some(body) = body {
            let _ = inference.constrain_expr_infer_ty(body, ret);
        }

        Ok(true)
    }

    fn callable_goal_args<'goal>(
        &self,
        goal: &'goal TraitGoal,
    ) -> Result<Option<(&'goal [InferTy], &'goal InferTy)>, PackageStoreError> {
        let Some(trait_data) = self.context.item_query().trait_data(goal.trait_ref)? else {
            return Ok(None);
        };
        if !matches!(trait_data.name.as_str(), "Fn" | "FnMut" | "FnOnce") {
            return Ok(None);
        }

        let [InferGenericArg::FnTraitArgs { params, ret }] = goal.args.as_slice() else {
            return Ok(None);
        };
        Ok(Some((params, ret)))
    }

    fn closure_expr(inference: &BodyInferenceCtx, ty: &InferTy) -> Option<ExprId> {
        let InferTy::Closure(id) = inference.root_resolved_ty(ty) else {
            return None;
        };
        Some(id.into_expr_id())
    }
}

#[cfg(test)]
mod tests {
    use rg_def_map::PackageSlot;
    use rg_ir_model::{BindingId, BodyId, BodyRef, TargetRef, TraitRef, TypeDefRef};
    use rg_package_store::PackageLoader;
    use rg_parse::TargetId;
    use rg_ty::{
        NominalTy, TraitGoal, Ty,
        inference::{InferGenericArg, InferTy},
    };

    use super::*;
    use crate::{ResolvedBodyData, testonly::BodyIrFixture};

    const FIXTURE: &str = r#"
//- /Cargo.toml
[package]
name = "body_callable_goal_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait FnOnce {}
pub trait NotCallable {}

pub struct User;
pub struct Name;

pub fn use_it(seed: Name) {
    let _closure = |user| seed;
}
"#;

    #[test]
    fn callable_goal_constrains_closure_param_and_body() {
        let fixture = GoalFixture::new();
        let mut inference = fixture.inference();
        let goal = fixture.callable_goal(
            &inference,
            vec![InferTy::from_ty(&fixture.user_ty())],
            InferTy::from_ty(&fixture.name_ty()),
        );

        assert!(
            fixture
                .solve_goal(&mut inference, &goal)
                .expect("callable goal should solve")
        );

        assert_eq!(
            inference.finalize_binding_ty(fixture.closure_param_binding()),
            fixture.user_ty()
        );
        assert_eq!(
            inference.finalize_expr_ty(fixture.closure_body()),
            fixture.name_ty()
        );
    }

    #[test]
    fn callable_goal_solves_generic_return_from_closure_body() {
        let fixture = GoalFixture::new();
        let mut inference = fixture.inference();
        let ret = inference.table.new_type_var();
        let goal = fixture.callable_goal(
            &inference,
            vec![InferTy::from_ty(&fixture.user_ty())],
            ret.clone(),
        );

        assert!(
            fixture
                .solve_goal(&mut inference, &goal)
                .expect("callable goal should solve")
        );

        assert_eq!(inference.table.finalize(&ret), fixture.name_ty());
    }

    #[test]
    fn non_callable_trait_goal_does_not_touch_closure() {
        let fixture = GoalFixture::new();
        let mut inference = fixture.inference();
        let goal = TraitGoal {
            self_ty: inference.expr_ty(fixture.closure_expr()),
            trait_ref: fixture.trait_ref("NotCallable"),
            args: Vec::new(),
        };

        assert!(
            !fixture
                .solve_goal(&mut inference, &goal)
                .expect("non-callable goal should not fail")
        );

        assert_eq!(
            inference.finalize_binding_ty(fixture.closure_param_binding()),
            Ty::Unknown
        );
    }

    #[test]
    fn callable_arity_mismatch_is_no_evidence() {
        let fixture = GoalFixture::new();
        let mut inference = fixture.inference();
        let ret = inference.table.new_type_var();
        let goal = fixture.callable_goal(&inference, Vec::new(), ret.clone());

        assert!(
            fixture
                .solve_goal(&mut inference, &goal)
                .expect("callable goal should be recognized")
        );

        assert_eq!(
            inference.finalize_binding_ty(fixture.closure_param_binding()),
            Ty::Unknown
        );
        assert_eq!(inference.table.finalize(&ret), Ty::Unknown);
    }

    struct GoalFixture {
        project: BodyIrFixture,
        target: TargetRef,
        body_ref: BodyRef,
    }

    impl GoalFixture {
        fn new() -> Self {
            let project = BodyIrFixture::build(FIXTURE);
            let target = TargetRef {
                package: PackageSlot(0),
                target: TargetId(0),
            };
            let body_ref = BodyRef {
                target,
                body: BodyId(0),
            };
            Self {
                project,
                target,
                body_ref,
            }
        }

        fn solve_goal(
            &self,
            inference: &mut BodyInferenceCtx,
            goal: &TraitGoal,
        ) -> Result<bool, PackageStoreError> {
            let def_maps = self
                .project
                .def_map_db()
                .read_txn(PackageLoader::resident_only("callable goal def maps"));
            let item_stores = self
                .project
                .semantic_ir_db()
                .read_txn(PackageLoader::resident_only("callable goal item stores"));
            let body_ir = self
                .project
                .body_ir_db()
                .read_txn(PackageLoader::resident_only("callable goal body ir"));
            let target_bodies = body_ir
                .target_bodies(self.target)
                .expect("target bodies should load")
                .expect("target bodies should exist");
            let body = target_bodies
                .body(self.body_ref.body)
                .expect("body should exist");
            let context = BodyResolutionContext::new(
                &def_maps,
                &item_stores,
                self.body_ref,
                body,
                target_bodies.semantic_index(),
            );
            BodyCallableGoalSolver::new(context).solve_goal(inference, goal)
        }

        fn body(&self) -> &ResolvedBodyData {
            self.project
                .resident_body(self.body_ref)
                .expect("fixture body should exist")
        }

        fn inference(&self) -> BodyInferenceCtx {
            let body = self.body();
            let mut inference = BodyInferenceCtx::new(body.exprs().len(), body.bindings().len());
            for expr_idx in 0..body.exprs().len() {
                let expr = ExprId(expr_idx);
                inference.set_expr_ty(expr, body.expr_ty_unchecked(expr));
            }
            for binding_idx in 0..body.bindings().len() {
                let binding = BindingId(binding_idx);
                inference.set_binding_ty(binding, body.binding_ty_unchecked(binding));
            }
            inference
        }

        fn callable_goal(
            &self,
            inference: &BodyInferenceCtx,
            params: Vec<InferTy>,
            ret: InferTy,
        ) -> TraitGoal {
            TraitGoal {
                self_ty: inference.expr_ty(self.closure_expr()),
                trait_ref: self.trait_ref("FnOnce"),
                args: vec![InferGenericArg::FnTraitArgs {
                    params,
                    ret: Box::new(ret),
                }],
            }
        }

        fn closure_expr(&self) -> ExprId {
            self.body()
                .exprs()
                .iter()
                .enumerate()
                .find_map(|(idx, expr)| {
                    matches!(&expr.kind, ExprKind::Closure { .. }).then_some(ExprId(idx))
                })
                .expect("fixture should contain a closure")
        }

        fn closure_param_binding(&self) -> BindingId {
            let ExprKind::Closure { params, .. } =
                self.body().expr_unchecked(self.closure_expr()).kind.clone()
            else {
                panic!("fixture closure expr should still be a closure");
            };
            params
                .first()
                .and_then(|param| param.bindings.first())
                .copied()
                .expect("fixture closure should have one binding param")
        }

        fn closure_body(&self) -> ExprId {
            let ExprKind::Closure { body, .. } =
                self.body().expr_unchecked(self.closure_expr()).kind
            else {
                panic!("fixture closure expr should still be a closure");
            };
            body.expect("fixture closure should have a body")
        }

        fn user_ty(&self) -> Ty {
            Ty::nominal(NominalTy::bare(self.type_def("User")))
        }

        fn name_ty(&self) -> Ty {
            Ty::nominal(NominalTy::bare(self.type_def("Name")))
        }

        fn type_def(&self, name: &str) -> TypeDefRef {
            let item_store = self
                .project
                .resident_target_ir(self.target)
                .expect("target item store should exist");
            item_store
                .structs()
                .iter_with_ids()
                .find_map(|(id, data)| {
                    (data.name.as_str() == name)
                        .then_some(TypeDefRef::new_struct(item_store.origin(), id))
                })
                .expect("fixture type should exist")
        }

        fn trait_ref(&self, name: &str) -> TraitRef {
            let item_store = self
                .project
                .resident_target_ir(self.target)
                .expect("target item store should exist");
            item_store
                .traits_with_refs()
                .find_map(|(trait_ref, data)| (data.name.as_str() == name).then_some(trait_ref))
                .expect("fixture trait should exist")
        }
    }
}
