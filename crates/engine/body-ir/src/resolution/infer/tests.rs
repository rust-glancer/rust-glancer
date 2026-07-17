use rg_ir_model::{
    BindingId, CrateId, CrateRef, DefMapRef, ExprId, PackageSlot, StructId, TypeDefId, TypeDefRef,
};
use rg_ty::{AdtTy, ClosureTyId, GenericArg, PrimitiveTy, Ty, UnsignedIntTy};

use super::context::BodyInferenceCtx;

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
    Ty::closure(ClosureTyId::new(ExprId(index)))
}

fn default_int_ty() -> Ty {
    Ty::Primitive(PrimitiveTy::DEFAULT_INT)
}

fn u64_ty() -> Ty {
    Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
}

#[test]
fn stores_closure_types_as_body_local_facts() {
    let mut context = BodyInferenceCtx::new(1, 0, 0);

    assert!(context.set_expr_closure_ty(ExprId(0)));

    assert_eq!(
        context.expr_ty(ExprId(0)),
        Ty::Closure(ClosureTyId::new(ExprId(0)))
    );
    assert_eq!(context.finalize_expr_ty(ExprId(0)), closure_ty(0));
}

#[test]
fn copies_closure_types_through_binding_reads() {
    let mut context = BodyInferenceCtx::new(2, 1, 0);

    context.set_expr_closure_ty(ExprId(0));
    context.set_binding_infer_ty(BindingId(0), context.expr_ty(ExprId(0)));

    assert!(context.set_expr_from_binding(ExprId(1), BindingId(0)));
    assert_eq!(
        context.expr_ty(ExprId(1)),
        Ty::Closure(ClosureTyId::new(ExprId(0)))
    );
    assert_eq!(context.finalize_expr_ty(ExprId(1)), closure_ty(0));
}

#[test]
fn creates_body_inference_context_with_body_sized_slots() {
    let mut context = BodyInferenceCtx::new(2, 3, 0);

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
    let mut context = BodyInferenceCtx::new(1, 0, 0);
    let var = context.table.new_type_var();

    context.set_expr_infer_ty(ExprId(0), var);
    assert_eq!(context.finalize_expr_ty(ExprId(0)), Ty::Unknown);

    assert!(context.constrain_expr_ty(ExprId(0), &user_ty()));
    assert_eq!(context.finalize_expr_ty(ExprId(0)), user_ty());
}

#[test]
fn expected_type_seeds_an_expression_without_producer_evidence() {
    let mut context = BodyInferenceCtx::new(1, 0, 0);

    assert!(context.constrain_expr_ty(ExprId(0), &user_ty()));

    assert_eq!(context.finalize_expr_ty(ExprId(0)), user_ty());
}

#[test]
fn binding_path_equality_carries_early_expected_type_back_to_the_binding() {
    let mut context = BodyInferenceCtx::new(1, 1, 0);
    context.constrain_expr_ty(ExprId(0), &vec_ty(user_ty()));

    assert!(context.set_expr_from_binding(ExprId(0), BindingId(0)));

    assert_eq!(context.finalize_binding_ty(BindingId(0)), vec_ty(user_ty()));
}

#[test]
fn revisiting_numeric_literals_keeps_their_inference_slots() {
    let mut context = BodyInferenceCtx::new(2, 0, 0);

    context.set_expr_integer_var(ExprId(0));
    context.set_expr_float_var(ExprId(1));
    let integer_slot = context.expr_ty(ExprId(0));
    let float_slot = context.expr_ty(ExprId(1));

    context.set_expr_integer_var(ExprId(0));
    context.set_expr_float_var(ExprId(1));

    assert_eq!(context.expr_ty(ExprId(0)), integer_slot);
    assert_eq!(context.expr_ty(ExprId(1)), float_slot);
}

#[test]
fn weaker_expression_evidence_does_not_create_fixed_point_progress() {
    let mut context = BodyInferenceCtx::new(1, 0, 0);
    context.set_expr_infer_ty(ExprId(0), user_ty());
    let before = context.progress();

    context.set_expr_infer_ty(ExprId(0), Ty::Unknown);

    assert!(!context.has_progressed_since(&before));
}

#[test]
fn fixed_point_ignores_fresh_ids_for_the_same_inference_shape() {
    let mut context = BodyInferenceCtx::new(1, 0, 0);
    let first = context.table.new_type_var();
    context.set_expr_infer_ty(ExprId(0), first);
    let before = context.progress();

    let replacement = context.table.new_type_var();
    context.set_expr_infer_ty(ExprId(0), replacement);

    assert!(!context.has_progressed_since(&before));
}

#[test]
fn weaker_pattern_evidence_does_not_replace_a_settled_binding_fact() {
    let mut context = BodyInferenceCtx::new(0, 1, 0);
    let settled = Ty::tuple(vec![user_ty(), user_ty()]);
    context.set_binding_infer_ty(BindingId(0), settled.clone());
    let before = context.progress();

    context.set_binding_infer_ty(BindingId(0), Ty::tuple(vec![user_ty(), Ty::Unknown]));

    assert!(!context.has_progressed_since(&before));
    assert_eq!(context.finalize_binding_ty(BindingId(0)), settled);
}

#[test]
fn conflicting_evidence_keeps_the_stable_slot_and_finalizes_to_unknown() {
    let mut context = BodyInferenceCtx::new(1, 0, 0);
    let slot = context.table.new_type_var();
    context.set_expr_infer_ty(ExprId(0), slot);

    context.set_expr_infer_ty(ExprId(0), user_ty());
    context.set_expr_infer_ty(ExprId(0), default_int_ty());

    assert_eq!(context.finalize_expr_ty(ExprId(0)), Ty::Unknown);
}

#[test]
fn treats_equivalent_variable_aliases_as_stable_body_facts() {
    let mut context = BodyInferenceCtx::new(1, 1, 0);
    let original = context.table.new_type_var();
    let alias = context.table.new_type_var();
    let unrelated = context.table.new_type_var();

    context.set_binding_infer_ty(BindingId(0), original.clone());
    context.set_expr_infer_ty(ExprId(0), original.clone());

    assert!(context.set_binding_infer_ty(BindingId(0), alias.clone()));
    assert!(!context.set_binding_infer_ty(BindingId(0), original));
    assert!(!context.set_expr_from_binding(ExprId(0), BindingId(0)));

    assert!(context.set_expr_infer_ty(ExprId(0), unrelated));
    assert!(!context.set_expr_from_binding(ExprId(0), BindingId(0)));
}

#[test]
fn empty_tuple_expression_is_unit_during_inference() {
    let mut context = BodyInferenceCtx::new(1, 0, 0);

    context.set_expr_tuple_from_fields(ExprId(0), &[]);

    assert_eq!(context.expr_ty(ExprId(0)), Ty::Unit);
    assert_eq!(context.finalize_expr_ty(ExprId(0)), Ty::Unit);
}

#[test]
fn revisiting_array_shapes_reuses_existing_element_slot() {
    let mut context = BodyInferenceCtx::new(3, 0, 0);
    let first_element = context.table.new_type_var();
    let second_element = context.table.new_type_var();
    context.set_expr_infer_ty(ExprId(0), first_element);
    context.set_expr_infer_ty(ExprId(1), second_element);

    context.set_expr_array_from_elements(ExprId(2), &[ExprId(0), ExprId(1)], Some("2".into()));
    let first = context.expr_ty(ExprId(2));

    context.set_expr_array_from_elements(ExprId(2), &[ExprId(0), ExprId(1)], Some("2".into()));

    assert_eq!(context.expr_ty(ExprId(2)), first);
}

#[test]
fn revisiting_array_shapes_keeps_new_slot_for_weak_evidence() {
    let mut context = BodyInferenceCtx::new(2, 0, 0);
    context.set_expr_ty(ExprId(0), &vec_ty(Ty::Unknown));

    context.set_expr_array_from_elements(ExprId(1), &[ExprId(0)], Some("1".into()));

    assert!(context.expr_ty(ExprId(1)).has_var());
    assert_eq!(
        context.finalize_expr_ty(ExprId(1)),
        Ty::Array {
            inner: Box::new(vec_ty(Ty::Unknown)),
            len: rg_ty::ConstValue::Scalar(1),
        }
    );
}

#[test]
fn revisiting_branch_shapes_reuses_existing_result_slot() {
    let mut context = BodyInferenceCtx::new(3, 0, 0);
    let then_ty = context.table.new_type_var();
    let else_ty = context.table.new_type_var();
    context.set_expr_infer_ty(ExprId(0), then_ty);
    context.set_expr_infer_ty(ExprId(1), else_ty);

    context.set_expr_if_from_branches(ExprId(2), Some(ExprId(0)), Some(ExprId(1)));
    let first = context.expr_ty(ExprId(2));

    context.set_expr_if_from_branches(ExprId(2), Some(ExprId(0)), Some(ExprId(1)));

    assert_eq!(context.expr_ty(ExprId(2)), first);
}

#[test]
fn revisiting_branch_shapes_does_not_reuse_concrete_fallback_result() {
    let mut context = BodyInferenceCtx::new(3, 0, 0);
    context.set_expr_integer_var(ExprId(0));
    context.set_expr_integer_var(ExprId(1));
    context.set_expr_ty(ExprId(2), &default_int_ty());

    context.set_expr_if_from_branches(ExprId(2), Some(ExprId(0)), Some(ExprId(1)));

    assert!(context.expr_ty(ExprId(2)).has_var());
    context.constrain_expr_ty(ExprId(2), &u64_ty());

    assert_eq!(context.finalize_expr_ty(ExprId(0)), u64_ty());
    assert_eq!(context.finalize_expr_ty(ExprId(1)), u64_ty());
    assert_eq!(context.finalize_expr_ty(ExprId(2)), u64_ty());
}

#[test]
fn revisiting_array_shapes_does_not_reuse_concrete_fallback_element() {
    let mut context = BodyInferenceCtx::new(3, 0, 0);
    context.set_expr_integer_var(ExprId(0));
    context.set_expr_integer_var(ExprId(1));
    context.set_expr_ty(
        ExprId(2),
        &Ty::Array {
            inner: Box::new(default_int_ty()),
            len: rg_ty::ConstValue::Scalar(2),
        },
    );

    context.set_expr_array_from_elements(ExprId(2), &[ExprId(0), ExprId(1)], Some("2".into()));

    assert!(context.expr_ty(ExprId(2)).has_var());
    context.constrain_expr_ty(
        ExprId(2),
        &Ty::Array {
            inner: Box::new(u64_ty()),
            len: rg_ty::ConstValue::Scalar(2),
        },
    );

    assert_eq!(context.finalize_expr_ty(ExprId(0)), u64_ty());
    assert_eq!(context.finalize_expr_ty(ExprId(1)), u64_ty());
    assert_eq!(
        context.finalize_expr_ty(ExprId(2)),
        Ty::Array {
            inner: Box::new(u64_ty()),
            len: rg_ty::ConstValue::Scalar(2),
        },
    );
}
