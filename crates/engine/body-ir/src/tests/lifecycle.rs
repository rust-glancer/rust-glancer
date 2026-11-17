use rg_ir_model::ExprId;
use rg_ty::{ConstValue, GenericArg};
use rg_workspace::TargetKind;

use crate::{ExprKind, testonly::BodyIrFixture};

#[test]
fn finalized_bodies_have_aligned_structural_and_semantic_arenas() {
    let mut fixture = BodyIrFixture::build(
        r#"
//- /src/lib.rs crate:body_lifecycle_fixture
fn identity<'a, T, const N: usize>(value: &'a [T; N]) -> &'a [T; N] { value }

pub fn use_it(value: &[u8; 3]) {
    let inferred: _ = identity::<u8, 3>(value);
}
"#,
    );
    fixture.inspect(|view| {
        let target = view
            .semantic()
            .crate_ref("body_lifecycle_fixture", TargetKind::Lib);
        let bodies = view.bodies(target);

        let mut selected_calls = Vec::new();
        for (_, body) in bodies {
            assert_eq!(body.exprs().len(), body.expr_facts().len());
            for (expr_idx, data) in body.exprs().iter().enumerate() {
                let expr = ExprId(expr_idx);
                if matches!(
                    data.kind,
                    ExprKind::Call { .. } | ExprKind::MethodCall { .. }
                ) && let Some(call) = body.call_facts(expr)
                {
                    selected_calls.push(call);
                }
            }
        }

        let [call] = selected_calls.as_slice() else {
            panic!("fixture should persist exactly one selected call")
        };
        assert!(matches!(
            call.generic_args().as_slice(),
            [
                GenericArg::Lifetime(_),
                GenericArg::Type(_),
                GenericArg::Const(ConstValue::Scalar(3)),
            ]
        ));
    });
}
