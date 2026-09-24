use rg_ir_model::{CrateId, CrateRef, ExprId, PackageSlot};
use rg_ty::{ConstValue, GenericArg};

use crate::{ExprKind, testonly::BodyIrFixture};

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
    for (_, body) in crate_bodies.body_views() {
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
}

#[test]
fn alias_placeholders_share_evidence_and_generic_calls_stay_independent() {
    use rg_ir_model::{BindingId, PrimitiveTy, UnsignedIntTy};
    use rg_ty::Ty;

    let fixture = BodyIrFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "source_type_identity"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
type Pair<T> = (T, T);
fn missing<T>() -> T { loop {} }
fn identity<T>(value: T) -> T { value }
fn require(_: u16) {}

pub fn use_it() {
    let pair: Pair<_> = missing();
    let right = pair.1;
    require(pair.0);
    let byte = identity(1u8);
    let flag = identity(true);
}
"#,
    );
    let bodies = fixture
        .body_ir_db()
        .resident_package(PackageSlot(0))
        .expect("fixture package")
        .crate_bodies(CrateId(0))
        .expect("fixture bodies");
    let u16_ty = Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U16));
    let expected = [
        ("pair", Ty::tuple(vec![u16_ty.clone(), u16_ty.clone()])),
        ("right", u16_ty),
        (
            "byte",
            Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U8)),
        ),
        ("flag", Ty::Primitive(PrimitiveTy::Bool)),
    ];
    for (name, expected) in expected {
        let actual = bodies
            .body_views()
            .find_map(|(_, body)| {
                body.bindings()
                    .iter()
                    .enumerate()
                    .find_map(|(index, binding)| {
                        (binding.name.as_deref() == Some(name))
                            .then(|| body.binding_ty(BindingId(index)).cloned())
                            .flatten()
                    })
            })
            .expect("named fixture binding");
        assert_eq!(actual, expected, "type of {name}");
    }
}
