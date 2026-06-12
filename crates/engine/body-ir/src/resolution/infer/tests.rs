use rg_ir_model::{
    DefMapRef, ExprId, StructId, TargetRef, TypeDefId, TypeDefRef,
    items::{FloatTy, SignedIntTy, UnsignedIntTy},
};
use rg_parse::TargetId;
use rg_ty::{GenericArg, NominalTy, PrimitiveTy, Ty};

use super::{
    context::BodyInferenceCtx,
    model::{InferGenericArg, InferNominalTy, InferTy},
    table::InferenceTable,
};

fn type_def(index: usize) -> TypeDefRef {
    TypeDefRef {
        origin: DefMapRef::Target(TargetRef {
            package: rg_def_map::PackageSlot(0),
            target: TargetId(0),
        }),
        id: TypeDefId::Struct(StructId(index)),
    }
}

fn user_ty() -> Ty {
    Ty::nominal(NominalTy::bare(type_def(0)))
}

fn project_ty() -> Ty {
    Ty::nominal(NominalTy::bare(type_def(1)))
}

fn vec_ty(inner: InferTy) -> InferTy {
    InferTy::Nominal(InferNominalTy {
        def: type_def(10),
        args: vec![InferGenericArg::Type(Box::new(inner))],
    })
}

#[test]
fn creates_body_inference_context_with_body_sized_slots() {
    let mut context = BodyInferenceCtx::new(2, 3);

    let var = context.table.new_type_var();

    assert_eq!(context.expr_tys, vec![InferTy::Unknown; 2]);
    assert_eq!(context.binding_tys, vec![InferTy::Unknown; 3]);
    assert_eq!(context.table.finalize(&var), Ty::Unknown);
}

#[test]
fn stores_expression_type_variables_until_expected_type_evidence_arrives() {
    let mut context = BodyInferenceCtx::new(1, 0);

    context.expr_tys[0] = context.table.new_type_var();
    assert_eq!(context.finalize_expr_ty(ExprId(0)), Ty::Unknown);

    assert!(context.constrain_expr_ty(ExprId(0), &user_ty()));
    assert_eq!(context.finalize_expr_ty(ExprId(0)), user_ty());
}

#[test]
fn finalizes_unsolved_variables_to_stable_fallbacks() {
    let mut table = InferenceTable::new();

    let ty_var = table.new_type_var();
    let int_var = table.new_integer_var();
    let float_var = table.new_float_var();

    assert_eq!(table.finalize(&ty_var), Ty::Unknown);
    assert_eq!(
        table.finalize(&int_var),
        Ty::Primitive(PrimitiveTy::SignedInt(SignedIntTy::I32))
    );
    assert_eq!(
        table.finalize(&float_var),
        Ty::Primitive(PrimitiveTy::Float(FloatTy::F64))
    );
}

#[test]
fn finalizes_solved_variables_inside_nominal_containers() {
    let mut table = InferenceTable::new();
    let element = table.new_type_var();

    assert!(table.unify(&element, &InferTy::from_ty(&user_ty())));

    assert_eq!(
        table.finalize(&vec_ty(element)),
        Ty::nominal(NominalTy {
            def: type_def(10),
            args: vec![GenericArg::Type(Box::new(user_ty()))],
        })
    );
}

#[test]
fn unifies_same_definition_nominal_generic_arguments() {
    let mut table = InferenceTable::new();
    let element = table.new_type_var();

    assert!(table.unify(
        &vec_ty(element.clone()),
        &vec_ty(InferTy::from_ty(&user_ty()))
    ));

    assert_eq!(
        table.finalize(&element),
        Ty::nominal(NominalTy::bare(type_def(0)))
    );
}

#[test]
fn conflicting_variables_finalize_to_unknown() {
    let mut table = InferenceTable::new();
    let var = table.new_type_var();

    assert!(table.unify(&var, &InferTy::from_ty(&user_ty())));
    assert!(table.unify(&var, &InferTy::from_ty(&project_ty())));

    assert_eq!(table.finalize(&var), Ty::Unknown);
}

#[test]
fn unknown_does_not_solve_variables() {
    let mut table = InferenceTable::new();
    let var = table.new_type_var();

    assert!(!table.unify(&var, &InferTy::Unknown));

    assert_eq!(table.finalize(&var), Ty::Unknown);
}

#[test]
fn numeric_variables_accept_matching_primitive_evidence() {
    let mut table = InferenceTable::new();
    let int_var = table.new_integer_var();
    let float_var = table.new_float_var();

    assert!(table.unify(
        &int_var,
        &InferTy::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
    ));
    assert!(table.unify(
        &float_var,
        &InferTy::Primitive(PrimitiveTy::Float(FloatTy::F32))
    ));

    assert_eq!(
        table.finalize(&int_var),
        Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
    );
    assert_eq!(
        table.finalize(&float_var),
        Ty::Primitive(PrimitiveTy::Float(FloatTy::F32))
    );
}

#[test]
fn numeric_variables_follow_already_solved_type_variables() {
    let mut table = InferenceTable::new();
    let type_var = table.new_type_var();
    let int_var = table.new_integer_var();

    assert!(table.unify(
        &type_var,
        &InferTy::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
    ));
    assert!(table.unify(&int_var, &type_var));

    assert_eq!(
        table.finalize(&int_var),
        Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
    );
    assert_eq!(
        table.finalize(&type_var),
        Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
    );
}
