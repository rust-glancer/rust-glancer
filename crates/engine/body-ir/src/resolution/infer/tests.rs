use rg_ir_model::{DefMapRef, ExprId, PackageSlot, StructId, TargetRef, TypeDefId, TypeDefRef};
use rg_parse::TargetId;
use rg_ty::{NominalTy, Ty, inference::InferTy};

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
