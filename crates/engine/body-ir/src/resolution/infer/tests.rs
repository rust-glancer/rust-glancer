use std::cell::Cell;

use rg_ir_model::{
    BindingId, BodyId, BodyRef, CrateId, CrateRef, DefMapRef, ExprId, PackageSlot, StructId,
    TypeDefId, TypeDefRef,
};
use rg_std::CancellationToken;
use rg_ty::{AdtTy, ClosureTyId, GenericArg, PrimitiveTy, Ty};

use super::unify::InferenceState;

fn type_def(index: usize) -> TypeDefRef {
    TypeDefRef {
        origin: DefMapRef::Crate(CrateRef {
            package: PackageSlot(0),
            crate_id: CrateId(0),
        }),
        id: TypeDefId::Struct(StructId(index)),
    }
}

fn user_ty() -> Ty {
    Ty::adt(AdtTy::bare(type_def(0)))
}

fn vec_ty(inner: Ty) -> Ty {
    Ty::adt(AdtTy {
        def: type_def(1),
        args: vec![GenericArg::Type(Box::new(inner))].into(),
    })
}

fn closure_ty(index: usize) -> Ty {
    Ty::closure(
        ClosureTyId::new(body_ref(), ExprId(index)),
        Vec::new(),
        Ty::Unknown,
    )
}

fn body_ref() -> BodyRef {
    BodyRef {
        crate_ref: CrateRef {
            package: PackageSlot(0),
            crate_id: CrateId(0),
        },
        body: BodyId(0),
    }
}

fn default_int_ty() -> Ty {
    Ty::Primitive(PrimitiveTy::DEFAULT_INT)
}

#[test]
fn stores_closure_types_as_body_local_facts() {
    let mut context = InferenceState::new(1, 0);

    context.set_expr_closure_ty(body_ref(), ExprId(0), 0);

    let Ty::Closure(closure) = context.expr_ty(ExprId(0)) else {
        panic!("closure expression should retain its callable signature");
    };
    assert_eq!(closure.id, ClosureTyId::new(body_ref(), ExprId(0)));
    assert!(closure.params.is_empty());
    assert!(closure.ret.has_var());
    assert_eq!(context.finalize_expr_ty(ExprId(0)), closure_ty(0));
}

#[test]
fn copies_closure_types_through_binding_reads() {
    let mut context = InferenceState::new(2, 1);

    context.set_expr_closure_ty(body_ref(), ExprId(0), 0);
    context.set_binding_infer_ty(BindingId(0), context.expr_ty(ExprId(0)));

    context.set_expr_from_binding(ExprId(1), BindingId(0));
    let Ty::Closure(closure) = context.expr_ty(ExprId(1)) else {
        panic!("binding reads should preserve closure identity and signature");
    };
    assert_eq!(closure.id, ClosureTyId::new(body_ref(), ExprId(0)));
    assert_eq!(context.finalize_expr_ty(ExprId(1)), closure_ty(0));
}

#[test]
fn creates_body_inference_context_with_body_sized_slots() {
    let mut context = InferenceState::new(2, 3);

    let var = context.table.new_type_var();

    assert_eq!(context.expr_ty(ExprId(0)), Ty::Unknown);
    assert_eq!(context.expr_ty(ExprId(1)), Ty::Unknown);
    assert_eq!(context.binding_ty(BindingId(0)), Ty::Unknown);
    assert_eq!(context.binding_ty(BindingId(1)), Ty::Unknown);
    assert_eq!(context.binding_ty(BindingId(2)), Ty::Unknown);
    assert_eq!(context.table.finalize(&var), Ty::Unknown);
}

#[test]
fn stores_expression_type_variables_until_expected_type_evidence_arrives() {
    let mut context = InferenceState::new(1, 0);
    let var = context.table.new_type_var();

    context.set_expr_infer_ty(ExprId(0), var);
    assert_eq!(context.finalize_expr_ty(ExprId(0)), Ty::Unknown);

    context.constrain_expr_ty(ExprId(0), &user_ty());
    assert_eq!(context.finalize_expr_ty(ExprId(0)), user_ty());
}

#[test]
fn expected_type_seeds_an_expression_without_producer_evidence() {
    let mut context = InferenceState::new(1, 0);

    context.constrain_expr_ty(ExprId(0), &user_ty());

    assert_eq!(context.finalize_expr_ty(ExprId(0)), user_ty());
}

#[test]
fn repeated_nested_unknown_instantiation_reuses_expression_slots() {
    let mut context = InferenceState::new(1, 0);
    let return_ty = vec_ty(Ty::Unknown);

    context.instantiate_expr_nested_unknown_ty(ExprId(0), &return_ty);
    let first_inference_ty = context.expr_ty(ExprId(0));

    context.instantiate_expr_nested_unknown_ty(ExprId(0), &return_ty);
    assert_eq!(context.expr_ty(ExprId(0)), first_inference_ty);
}

#[test]
fn never_expression_does_not_solve_its_expected_type_slot() {
    let mut context = InferenceState::new(1, 0);
    context.set_expr_infer_ty(ExprId(0), Ty::Never);
    let expected = context.table.new_type_var();

    context.constrain_expr_ty(ExprId(0), &expected);
    context.table.unify(&expected, &user_ty());

    assert_eq!(context.finalize_expr_ty(ExprId(0)), Ty::Never);
    assert_eq!(context.table.finalize(&expected), user_ty());
}

#[test]
fn binding_path_equality_carries_early_expected_type_back_to_the_binding() {
    let mut context = InferenceState::new(1, 1);
    context.constrain_expr_ty(ExprId(0), &vec_ty(user_ty()));

    context.set_expr_from_binding(ExprId(0), BindingId(0));

    assert_eq!(context.finalize_binding_ty(BindingId(0)), vec_ty(user_ty()));
}

#[test]
fn weaker_expression_evidence_preserves_known_type() {
    let mut context = InferenceState::new(1, 0);
    context.set_expr_infer_ty(ExprId(0), user_ty());

    context.set_expr_infer_ty(ExprId(0), Ty::Unknown);
    assert_eq!(context.finalize_expr_ty(ExprId(0)), user_ty());
}

#[test]
fn linked_expression_variables_share_later_evidence() {
    let mut context = InferenceState::new(1, 0);
    let first = context.table.new_type_var();
    context.set_expr_infer_ty(ExprId(0), first.clone());

    let replacement = context.table.new_type_var();
    context.set_expr_infer_ty(ExprId(0), replacement.clone());
    context.table.unify(&first, &user_ty());
    assert_eq!(context.table.finalize(&replacement), user_ty());
    assert_eq!(context.finalize_expr_ty(ExprId(0)), user_ty());
}

#[test]
fn weaker_pattern_evidence_does_not_replace_a_settled_binding_fact() {
    let mut context = InferenceState::new(0, 1);
    let settled = Ty::tuple(vec![user_ty(), user_ty()]);
    context.set_binding_infer_ty(BindingId(0), settled.clone());

    context.set_binding_infer_ty(BindingId(0), Ty::tuple(vec![user_ty(), Ty::Unknown]));

    assert_eq!(context.finalize_binding_ty(BindingId(0)), settled);
}

#[test]
fn conflicting_evidence_keeps_the_stable_slot_and_finalizes_to_unknown() {
    let mut context = InferenceState::new(1, 0);
    let slot = context.table.new_type_var();
    context.set_expr_infer_ty(ExprId(0), slot);

    context.set_expr_infer_ty(ExprId(0), user_ty());
    context.set_expr_infer_ty(ExprId(0), default_int_ty());

    assert_eq!(context.finalize_expr_ty(ExprId(0)), Ty::Unknown);
}

#[test]
fn treats_equivalent_variable_aliases_as_stable_body_facts() {
    let mut context = InferenceState::new(1, 1);
    let original = context.table.new_type_var();
    let alias = context.table.new_type_var();
    let unrelated = context.table.new_type_var();

    context.set_binding_infer_ty(BindingId(0), original.clone());
    context.set_expr_infer_ty(ExprId(0), original.clone());

    context.set_binding_infer_ty(BindingId(0), alias.clone());
    context.set_binding_infer_ty(BindingId(0), original.clone());
    context.set_expr_from_binding(ExprId(0), BindingId(0));

    context.set_expr_infer_ty(ExprId(0), unrelated.clone());
    context.set_expr_from_binding(ExprId(0), BindingId(0));

    context.table.unify(&alias, &user_ty());
    assert_eq!(context.table.finalize(&original), user_ty());
    assert_eq!(context.table.finalize(&unrelated), user_ty());
    assert_eq!(context.finalize_binding_ty(BindingId(0)), user_ty());
    assert_eq!(context.finalize_expr_ty(ExprId(0)), user_ty());
}

thread_local! {
    static CANCEL_AFTER_EXPRESSIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

pub(super) fn before_expression(cancellation: &CancellationToken) {
    CANCEL_AFTER_EXPRESSIONS.with(|remaining| {
        if let Some(count) = remaining.get() {
            remaining.set(count.checked_sub(1));
            if count == 0 {
                cancellation.cancel();
            }
        }
    });
}

#[test]
fn cancelling_recursive_inference_never_finalizes_partial_body_facts() {
    let fixture = crate::testonly::BodyIrFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "cancelled_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn compute() -> u32 { let first = 1_u32; let second = first + 2; second + 3 }
"#,
    );
    let target = CrateRef {
        package: PackageSlot(0),
        crate_id: CrateId(0),
    };
    let bodies = fixture
        .body_ir_db()
        .resident_package(target.package)
        .expect("fixture package exists")
        .crate_bodies(target.crate_id)
        .expect("fixture crate exists");
    let body = &bodies.bodies()[0];
    let def_map = fixture
        .def_map_db()
        .read_txn(rg_def_map::DefMapLoader::resident_only("inference fixture"));
    let semantic_ir =
        fixture
            .semantic_ir_db()
            .read_txn(rg_semantic_ir::SemanticIrLoader::resident_only(
                "inference fixture",
            ));
    let lookup = rg_semantic_ir::ItemLookupQuery::build_from(
        &rg_semantic_ir::CrateItemQuery::new(&def_map, &semantic_ir, target),
        &CancellationToken::new(),
    )
    .expect("fixture lookup builds");
    for cancel in [true, false] {
        let cancellation = CancellationToken::new();
        let session = rg_ty::trait_selection::TraitSelectionSession::new(target)
            .with_cancellation(cancellation);
        CANCEL_AFTER_EXPRESSIONS.with(|remaining| remaining.set(cancel.then_some(2)));
        let result = super::InferenceContext::new(
            &def_map,
            &semantic_ir,
            &lookup,
            BodyRef {
                crate_ref: target,
                body: BodyId(0),
            },
            body,
            &session,
        )
        .infer_body();
        if cancel {
            let error = result.expect_err("unfinished inference must have no facts");
            let cancelled = error
                .chain()
                .find_map(|cause| cause.downcast_ref::<rg_std::Cancelled>())
                .expect("inference preserves the cancellation cause");
            assert_eq!(cancelled.checkpoint(), "expression resolution");
        } else {
            let facts = result.expect("fresh inference can finish");
            assert_eq!(facts.exprs.len(), body.exprs().len());
            assert!(facts.exprs.iter().all(|facts| !facts.ty.has_var()));
        }
        assert!(CANCEL_AFTER_EXPRESSIONS.with(|remaining| remaining.get().is_none()));
    }
}
