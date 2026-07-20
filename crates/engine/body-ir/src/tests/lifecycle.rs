use rg_def_map::PackageSlot;
use rg_ir_model::{BodyRef, CrateId, CrateRef, ExprId};
use rg_ty::{ConstValue, GenericArg};

use crate::ExprKind;
use crate::testonly::BodyIrFixture;

#[test]
fn finalized_bodies_have_aligned_structural_and_semantic_arenas() {
    let fixture = BodyIrFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "body_lifecycle_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
fn identity<'a, T, const N: usize>(value: &'a [T; N]) -> &'a [T; N] { value }

pub fn use_it(value: &[u8; 3]) {
    let inferred: _ = identity::<u8, 3>(value);
}
"#,
    );
    let crate_ref = CrateRef {
        package: PackageSlot(0),
        crate_id: CrateId(0),
    };
    let crate_bodies = fixture
        .body_ir_db()
        .resident_package(crate_ref.package)
        .expect("fixture package should be resident")
        .crate_bodies(crate_ref.crate_id)
        .expect("fixture crate should have Body IR");

    let mut selected_calls = Vec::new();
    for (body_id, body) in crate_bodies.body_views() {
        assert_eq!(body.bindings().len(), body.binding_facts().len());
        assert_eq!(body.exprs().len(), body.expr_facts().len());
        assert!(
            body.expr_declarations(
                BodyRef {
                    crate_ref,
                    body: body_id,
                },
                ExprId(usize::MAX),
            )
            .is_empty(),
            "checked declaration lookup should tolerate a stale expression id",
        );

        // Inference variables belong to the resolution pass. Persisted facts expose only stable
        // semantic types, even when written `_` forced the pass to create a temporary slot.
        assert!(body.binding_facts().iter().all(|facts| !facts.ty.has_var()));
        assert!(body.expr_facts().iter().all(|facts| !facts.ty.has_var()));

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
            GenericArg::Type(ty),
            GenericArg::Const(ConstValue::Scalar(3)),
        ] if !ty.has_var()
    ));
}
