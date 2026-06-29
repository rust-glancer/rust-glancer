use rg_ir_model::{
    BindingId, DefMapRef, ExprId, PackageSlot, StructId, TargetRef, TypeDefId, TypeDefRef,
};
use rg_parse::TargetId;
use rg_ty::{
    ClosureTyId, GenericArg, NominalTy, PrimitiveTy, Ty, UnsignedIntTy, inference::InferTy,
};

use super::context::BodyInferenceCtx;

fn type_def(index: usize) -> TypeDefRef {
    TypeDefRef {
        origin: DefMapRef::Target(TargetRef {
            package: PackageSlot(0),
            target: TargetId(0),
        }),
        id: TypeDefId::Struct(StructId(index)),
    }
}

fn user_ty() -> Ty {
    Ty::nominal(NominalTy::bare(type_def(0)))
}

fn vec_ty(inner: Ty) -> Ty {
    Ty::nominal(NominalTy {
        def: type_def(1),
        args: vec![GenericArg::Type(Box::new(inner))],
    })
}

fn closure_ty(index: u32) -> Ty {
    Ty::closure(ClosureTyId::new(index))
}

fn default_int_ty() -> Ty {
    Ty::Primitive(PrimitiveTy::DEFAULT_INT)
}

fn u64_ty() -> Ty {
    Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
}

#[test]
fn stores_closure_types_as_body_local_facts() {
    let mut context = BodyInferenceCtx::new(1, 0);

    assert!(context.set_expr_closure_ty(ExprId(0)));

    assert_eq!(
        context.expr_ty(ExprId(0)),
        InferTy::Closure(ClosureTyId::new(0))
    );
    assert_eq!(context.finalize_expr_ty(ExprId(0)), closure_ty(0));
}

#[test]
fn copies_closure_types_through_binding_reads() {
    let mut context = BodyInferenceCtx::new(2, 1);

    context.set_expr_closure_ty(ExprId(0));
    context.set_binding_infer_ty(BindingId(0), context.expr_ty(ExprId(0)));

    assert!(context.set_expr_from_binding(ExprId(1), BindingId(0)));
    assert_eq!(
        context.expr_ty(ExprId(1)),
        InferTy::Closure(ClosureTyId::new(0))
    );
    assert_eq!(context.finalize_expr_ty(ExprId(1)), closure_ty(0));
}

#[test]
fn creates_body_inference_context_with_body_sized_slots() {
    let mut context = BodyInferenceCtx::new(2, 3);

    let var = context.table.new_type_var();

    assert_eq!(context.expr_ty(ExprId(0)), InferTy::Unknown);
    assert_eq!(context.expr_ty(ExprId(1)), InferTy::Unknown);
    assert_eq!(context.binding_ty(BindingId(0)), InferTy::Unknown);
    assert_eq!(context.binding_ty(BindingId(1)), InferTy::Unknown);
    assert_eq!(context.binding_ty(BindingId(2)), InferTy::Unknown);
    assert_eq!(context.table.finalize(&var), Ty::Unknown);
}

#[test]
fn stores_expression_type_variables_until_expected_type_evidence_arrives() {
    let mut context = BodyInferenceCtx::new(1, 0);
    let var = context.table.new_type_var();

    context.set_expr_infer_ty(ExprId(0), var);
    assert_eq!(context.finalize_expr_ty(ExprId(0)), Ty::Unknown);

    assert!(context.constrain_expr_ty(ExprId(0), &user_ty()));
    assert_eq!(context.finalize_expr_ty(ExprId(0)), user_ty());
}

#[test]
fn treats_equivalent_variable_aliases_as_stable_body_facts() {
    let mut context = BodyInferenceCtx::new(1, 1);
    let original = context.table.new_type_var();
    let alias = context.table.new_type_var();
    let unrelated = context.table.new_type_var();

    context.set_binding_infer_ty(BindingId(0), original.clone());
    context.set_expr_infer_ty(ExprId(0), original.clone());

    assert!(context.set_binding_infer_ty(BindingId(0), alias.clone()));
    assert!(!context.set_binding_infer_ty(BindingId(0), original));
    assert!(!context.set_expr_from_binding(ExprId(0), BindingId(0)));

    context.set_expr_infer_ty(ExprId(0), unrelated);
    assert!(context.set_expr_from_binding(ExprId(0), BindingId(0)));
    assert!(!context.set_expr_from_binding(ExprId(0), BindingId(0)));
}

#[test]
fn refreshing_array_shapes_reuses_existing_element_slot() {
    let mut context = BodyInferenceCtx::new(3, 0);
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
fn refreshing_array_shapes_keeps_new_slot_for_weak_evidence() {
    let mut context = BodyInferenceCtx::new(2, 0);
    context.set_expr_ty(ExprId(0), &vec_ty(Ty::Unknown));

    context.set_expr_array_from_elements(ExprId(1), &[ExprId(0)], Some("1".into()));

    assert!(context.expr_ty(ExprId(1)).has_var());
    assert_eq!(
        context.finalize_expr_ty(ExprId(1)),
        Ty::Array {
            inner: Box::new(vec_ty(Ty::Unknown)),
            len: Some("1".into()),
        }
    );
}

#[test]
fn refreshing_branch_shapes_reuses_existing_result_slot() {
    let mut context = BodyInferenceCtx::new(3, 0);
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
fn refreshing_branch_shapes_does_not_reuse_concrete_fallback_result() {
    let mut context = BodyInferenceCtx::new(3, 0);
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
fn refreshing_array_shapes_does_not_reuse_concrete_fallback_element() {
    let mut context = BodyInferenceCtx::new(3, 0);
    context.set_expr_integer_var(ExprId(0));
    context.set_expr_integer_var(ExprId(1));
    context.set_expr_ty(
        ExprId(2),
        &Ty::Array {
            inner: Box::new(default_int_ty()),
            len: Some("2".into()),
        },
    );

    context.set_expr_array_from_elements(ExprId(2), &[ExprId(0), ExprId(1)], Some("2".into()));

    assert!(context.expr_ty(ExprId(2)).has_var());
    context.constrain_expr_ty(
        ExprId(2),
        &Ty::Array {
            inner: Box::new(u64_ty()),
            len: Some("2".into()),
        },
    );

    assert_eq!(context.finalize_expr_ty(ExprId(0)), u64_ty());
    assert_eq!(context.finalize_expr_ty(ExprId(1)), u64_ty());
    assert_eq!(
        context.finalize_expr_ty(ExprId(2)),
        Ty::Array {
            inner: Box::new(u64_ty()),
            len: Some("2".into()),
        },
    );
}
