//! Callable trait goals that can be answered from body-local closure witnesses.
//!
//! This is the first bridge between the real closure type witness and trait obligations. It does
//! not try to prove capture semantics. For now, a closure witness can provide evidence for any of
//! `Fn`, `FnMut`, or `FnOnce` when the goal's argument shape fits the closure's written params.

use rg_def_map::DefMapSource;
use rg_ir_model::ExprId;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::{GenericArg, Substitution, TraitGoal, Ty, inference::InferVarKind};

use crate::{ir::ExprKind, resolution::BodyResolutionContext};

use super::super::{BodyInferenceCtx, BodyPatternInference};

/// Applies callable trait goals directly to body-local closure witnesses.
///
/// Shared trait selection cannot inspect a closure body because closure witnesses and their inference
/// slots exist only while that body is being resolved. This solver supplies that missing local evidence:
/// it relates an `Fn*` goal's input and output types to the closure's patterns and body expression, then
/// lets the surrounding obligation solver commit the resulting inference facts.
pub(super) struct BodyCallableGoalSolver<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BodyCallableGoalOutcome {
    /// The goal is not a supported `Fn*` application over a body-local callable witness.
    NotApplicable,
    /// The callable shape supplied all evidence this bounded solver needs.
    Solved,
    /// The closure is callable, but its body and expected output are both unresolved type slots.
    Deferred,
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
    /// Unsupported or malformed goals are `NotApplicable` so ordinary trait selection may still
    /// try to handle them. A callable arity mismatch is consumed but produces no evidence: we know
    /// this is the closure-callable approximation, but the written function shape does not fit
    /// this closure expression. `Deferred` keeps an otherwise valid goal available to the next
    /// fixed-point pass rather than committing an impl-only output variable with no durable body
    /// evidence.
    pub(super) fn solve_goal(
        &self,
        inference: &mut BodyInferenceCtx,
        goal: &TraitGoal,
    ) -> Result<BodyCallableGoalOutcome, PackageStoreError> {
        let Some((params, ret)) = self.callable_goal_args(goal)? else {
            return Ok(BodyCallableGoalOutcome::NotApplicable);
        };
        self.solve_fn_trait_goal(inference, goal.self_ty(), params, ret)
    }

    /// Use already-projected callable fn-trait args as closure-local inference evidence.
    pub(super) fn solve_fn_trait_goal(
        &self,
        inference: &mut BodyInferenceCtx,
        self_ty: &Ty,
        params: &[Ty],
        ret: &Ty,
    ) -> Result<BodyCallableGoalOutcome, PackageStoreError> {
        if let Ty::FnDef(function) = inference.root_resolved_ty(self_ty) {
            return Ok(
                if self.solve_fn_def_goal(inference, &function, params, ret)? {
                    BodyCallableGoalOutcome::Solved
                } else {
                    BodyCallableGoalOutcome::NotApplicable
                },
            );
        }

        let Some(closure) = Self::closure_expr(inference, self_ty) else {
            return Ok(BodyCallableGoalOutcome::NotApplicable);
        };
        let Some(closure_data) = self.context.body().expr(closure).cloned() else {
            return Ok(BodyCallableGoalOutcome::NotApplicable);
        };
        let ExprKind::Closure {
            params: closure_params,
            body,
            ..
        } = closure_data.kind
        else {
            return Ok(BodyCallableGoalOutcome::NotApplicable);
        };
        if closure_params.len() != params.len() {
            return Ok(BodyCallableGoalOutcome::Solved);
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
            if expected_ty.has_unknown() {
                continue;
            }
            let _ = pattern_inference.link_pat(inference, pat, &expected_ty);
        }

        // Return evidence flows in the opposite direction too: if `ret` is `?R` and the closure
        // body is known to be `Name`, this solves `?R = Name` for the caller that owns the goal.
        //
        // A body call can still be an unresolved ordinary type slot at this point. Linking two
        // such slots is not enough to finish the obligation: method resolution may replace the
        // temporary body fact later, leaving the impl-only return slot orphaned. Keep the goal
        // pending so the fixed-point pass retries it after the closure body gains a real shape.
        if let Some(body) = body {
            let body_ty = inference.root_resolved_expr_ty(body);
            let ret_ty = inference.root_resolved_ty(ret);
            let body_is_pending = matches!(
                body_ty,
                Ty::Unknown
                    | Ty::InferVar {
                        kind: InferVarKind::Type,
                        ..
                    }
            );
            let ret_is_pending = matches!(
                ret_ty,
                Ty::Unknown
                    | Ty::InferVar {
                        kind: InferVarKind::Type,
                        ..
                    }
            );
            if body_is_pending && ret_is_pending {
                return Ok(BodyCallableGoalOutcome::Deferred);
            }
            let _ = inference.constrain_expr_ty(body, ret);
        }

        Ok(BodyCallableGoalOutcome::Solved)
    }

    fn solve_fn_def_goal(
        &self,
        inference: &mut BodyInferenceCtx,
        function: &rg_ty::FnDefTy,
        params: &[Ty],
        ret: &Ty,
    ) -> Result<bool, PackageStoreError> {
        let Some(signature) = self.context.signatures().function(function.def)? else {
            return Ok(false);
        };
        if signature.params.len() != params.len() {
            return Ok(false);
        }

        let generics = self
            .context
            .item_paths()
            .generics()
            .generics(rg_ir_model::GenericDefRef::Function(function.def))?;
        let subst = Substitution::from_args(&generics, &function.args);
        for (param_ty, expected_param) in signature.params.iter().zip(params) {
            let param_ty = subst.apply(param_ty);
            if param_ty.has_unknown() {
                continue;
            }
            if inference
                .table
                .try_unify(&param_ty, expected_param)
                .is_err()
            {
                return Ok(false);
            }
        }

        let ret_ty = subst.apply(&signature.ret);
        if !ret_ty.has_unknown() && inference.table.try_unify(ret, &ret_ty).is_err() {
            return Ok(false);
        }

        Ok(true)
    }

    fn callable_goal_args<'goal>(
        &self,
        goal: &'goal TraitGoal,
    ) -> Result<Option<(&'goal [Ty], &'goal Ty)>, PackageStoreError> {
        let Some(trait_data) = self.context.item_query().trait_data(goal.trait_ref())? else {
            return Ok(None);
        };
        if !matches!(trait_data.name.as_str(), "Fn" | "FnMut" | "FnOnce") {
            return Ok(None);
        }

        let mut args = goal.iter_positional_args();
        let Some(GenericArg::Type(input)) = args.next() else {
            return Ok(None);
        };
        if args.next().is_some() {
            return Ok(None);
        }
        let params = match input.as_ref() {
            Ty::Unit => &[][..],
            Ty::Tuple(params) => params,
            _ => return Ok(None),
        };
        let mut ret = None;
        for binding in &goal.associated_types {
            let Some(data) = self
                .context
                .item_query()
                .type_alias_data(binding.associated_ty)?
            else {
                continue;
            };
            if data.name.as_str() == "Output" {
                ret = Some(&binding.ty);
                break;
            }
        }
        let Some(ret) = ret else {
            return Ok(None);
        };
        Ok(Some((params, ret)))
    }

    fn closure_expr(inference: &BodyInferenceCtx, ty: &Ty) -> Option<ExprId> {
        let Ty::Closure(id) = inference.root_resolved_ty(ty) else {
            return None;
        };
        Some(id.into_expr_id())
    }
}

#[cfg(test)]
mod tests {
    use rg_def_map::PackageSlot;
    use rg_ir_model::{
        AssocItemId, BindingId, BodyId, BodyRef, CrateId, CrateRef, TraitDefRef, TypeAliasRef,
        TypeDefRef,
    };
    use rg_package_store::PackageLoader;
    use rg_ty::{AdtTy, GenericArg, TraitGoal, Ty};

    use super::*;
    use crate::{BodyIrLoader, ResolvedBodyData, testonly::BodyIrFixture};

    const FIXTURE: &str = r#"
//- /Cargo.toml
[package]
name = "body_callable_goal_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait FnOnce { type Output; }
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
        let goal = fixture.callable_goal(&inference, vec![fixture.user_ty()], fixture.name_ty());

        assert_eq!(
            fixture
                .solve_goal(&mut inference, &goal)
                .expect("callable goal should solve"),
            BodyCallableGoalOutcome::Solved
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
        let goal = fixture.callable_goal(&inference, vec![fixture.user_ty()], ret.clone());

        assert_eq!(
            fixture
                .solve_goal(&mut inference, &goal)
                .expect("callable goal should solve"),
            BodyCallableGoalOutcome::Solved
        );

        assert_eq!(inference.table.finalize(&ret), fixture.name_ty());
    }

    #[test]
    fn non_callable_trait_goal_does_not_touch_closure() {
        let fixture = GoalFixture::new();
        let mut inference = fixture.inference();
        let goal = TraitGoal::new(
            inference.expr_ty(fixture.closure_expr()),
            fixture.trait_ref("NotCallable"),
            Vec::new(),
        );

        assert_eq!(
            fixture
                .solve_goal(&mut inference, &goal)
                .expect("non-callable goal should not fail"),
            BodyCallableGoalOutcome::NotApplicable
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

        assert_eq!(
            fixture
                .solve_goal(&mut inference, &goal)
                .expect("callable goal should be recognized"),
            BodyCallableGoalOutcome::Solved
        );

        assert_eq!(
            inference.finalize_binding_ty(fixture.closure_param_binding()),
            Ty::Unknown
        );
        assert_eq!(inference.table.finalize(&ret), Ty::Unknown);
    }

    #[test]
    fn callable_goal_waits_for_an_unresolved_closure_body() {
        let fixture = GoalFixture::new();
        let mut inference = fixture.inference();
        let unresolved_body = inference.table.new_type_var();
        inference.set_expr_infer_ty(fixture.closure_body(), unresolved_body);
        let ret = inference.table.new_type_var();
        let goal = fixture.callable_goal(&inference, vec![fixture.user_ty()], ret.clone());

        assert_eq!(
            fixture
                .solve_goal(&mut inference, &goal)
                .expect("callable goal should defer"),
            BodyCallableGoalOutcome::Deferred
        );
        assert_eq!(inference.table.finalize(&ret), Ty::Unknown);
    }

    struct GoalFixture {
        project: BodyIrFixture,
        crate_ref: CrateRef,
        body_ref: BodyRef,
    }

    impl GoalFixture {
        fn new() -> Self {
            let project = BodyIrFixture::build(FIXTURE);
            let crate_ref = CrateRef {
                package: PackageSlot(0),
                crate_id: CrateId(0),
            };
            let body_ref = BodyRef {
                crate_ref,
                body: BodyId(0),
            };
            Self {
                project,
                crate_ref,
                body_ref,
            }
        }

        fn solve_goal(
            &self,
            inference: &mut BodyInferenceCtx,
            goal: &TraitGoal,
        ) -> Result<BodyCallableGoalOutcome, PackageStoreError> {
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
                .read_txn(BodyIrLoader::resident_only("callable goal body ir"));
            let crate_bodies = body_ir
                .crate_bodies(self.crate_ref)
                .expect("crate bodies should load")
                .expect("crate bodies should exist");
            let body = crate_bodies
                .body(self.body_ref.body)
                .expect("body should exist");
            let context = BodyResolutionContext::new(
                &def_maps,
                &item_stores,
                self.body_ref,
                body,
                crate_bodies.semantic_index(),
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
            params: Vec<Ty>,
            ret: Ty,
        ) -> TraitGoal {
            let mut goal = TraitGoal::new(
                inference.expr_ty(self.closure_expr()),
                self.trait_ref("FnOnce"),
                vec![GenericArg::Type(Box::new(Ty::tuple(params)))],
            );
            goal.associated_types.push(rg_ty::AssocTypeBinding {
                associated_ty: self.output_alias(),
                ty: ret,
            });
            goal
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
            Ty::adt(AdtTy::bare(self.type_def("User")))
        }

        fn name_ty(&self) -> Ty {
            Ty::adt(AdtTy::bare(self.type_def("Name")))
        }

        fn type_def(&self, name: &str) -> TypeDefRef {
            let item_store = self
                .project
                .resident_crate_ir(self.crate_ref)
                .expect("crate item store should exist");
            item_store
                .structs()
                .iter_with_ids()
                .find_map(|(id, data)| {
                    (data.name.as_str() == name)
                        .then_some(TypeDefRef::new_struct(item_store.origin(), id))
                })
                .expect("fixture type should exist")
        }

        fn trait_ref(&self, name: &str) -> TraitDefRef {
            let item_store = self
                .project
                .resident_crate_ir(self.crate_ref)
                .expect("crate item store should exist");
            item_store
                .traits_with_refs()
                .find_map(|(trait_ref, data)| (data.name.as_str() == name).then_some(trait_ref))
                .expect("fixture trait should exist")
        }

        fn output_alias(&self) -> TypeAliasRef {
            let item_store = self
                .project
                .resident_crate_ir(self.crate_ref)
                .expect("crate item store should exist");
            let trait_ref = self.trait_ref("FnOnce");
            let trait_data = item_store
                .trait_data(trait_ref.id)
                .expect("FnOnce trait should exist");
            trait_data
                .items
                .iter()
                .find_map(|item| match item {
                    AssocItemId::TypeAlias(id) => Some(TypeAliasRef {
                        origin: trait_ref.origin,
                        id: *id,
                    }),
                    AssocItemId::Function(_) | AssocItemId::Const(_) => None,
                })
                .expect("FnOnce Output alias should exist")
        }
    }
}
