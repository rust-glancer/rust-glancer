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
fn selected_method_facts_keep_parent_args_and_later_inference() {
    use rg_ir_model::{BodyRef, PrimitiveTy, UnsignedIntTy, identity::DeclarationRef};
    use rg_ty::Ty;

    let fixture = BodyIrFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "selected_method_facts"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
struct Wrapper<'a, T, const N: usize> { value: &'a [T; N] }
impl<'a, T, const N: usize> Wrapper<'a, T, N> {
    fn make<U>(&self) -> (T, U) { loop {} }
}

pub fn use_it(wrapper: Wrapper<'_, u8, 3>) {
    let result = wrapper.make::<_>();
    let _: (u8, bool) = result;
}
"#,
    );
    let crate_ref = CrateRef {
        package: PackageSlot(0),
        crate_id: CrateId(0),
    };
    let function = fixture
        .resident_crate_ir(crate_ref)
        .expect("fixture items")
        .functions_with_refs()
        .find_map(|(function, data)| (data.name == "make").then_some(function))
        .expect("fixture method");
    let bodies = fixture
        .body_ir_db()
        .resident_package(crate_ref.package)
        .expect("fixture package")
        .crate_bodies(crate_ref.crate_id)
        .expect("fixture bodies");
    let (body_id, body, expr) = bodies
        .body_views()
        .find_map(|(body_id, body)| {
            body.exprs()
                .iter()
                .position(|expr| matches!(expr.kind, ExprKind::MethodCall { .. }))
                .map(|index| (body_id, body, ExprId(index)))
        })
        .expect("fixture method call");
    let call = body.call_facts(expr).expect("selected method call");

    // The impl arguments precede the method's U. U learns bool only from the later binding,
    // after argument checking has finished, and navigation must keep the selected method.
    let [
        GenericArg::Lifetime(_),
        GenericArg::Type(parent_ty),
        GenericArg::Const(ConstValue::Scalar(3)),
        GenericArg::Type(method_ty),
    ] = call.generic_args().as_slice()
    else {
        panic!("unexpected method arguments: {:?}", call.generic_args());
    };
    assert_eq!(
        parent_ty.as_ref(),
        &Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U8))
    );
    assert_eq!(method_ty.as_ref(), &Ty::Primitive(PrimitiveTy::Bool));
    assert_eq!(call.function(), function);
    let body_ref = BodyRef {
        crate_ref,
        body: body_id,
    };
    assert_eq!(
        body.expr_declarations(body_ref, expr),
        vec![DeclarationRef::from(function)]
    );
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
