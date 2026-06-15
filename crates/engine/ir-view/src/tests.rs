// Note: this module exists as a smoke test for `ViewFixture` mostly;
// the real coverage comes from `analysis` crate, since this crate is
// mostly a facade. At least for now, we don't expect to add many
// tests here, though it doesn't have usual snapshot-driven flow.

use rg_ir_model::{
    BodyOwner, ExprId, PackageSlot, TargetRef,
    identity::{DeclarationRef, ExprRef},
    items::{PrimitiveTy, SignedIntTy},
};
use rg_ty::Ty;

use crate::{lookup::resolution::ResolutionView, testonly::ViewFixture, ty::TyView};

#[test]
fn projects_body_expression_types_through_view_fixture() -> anyhow::Result<()> {
    let fixture = ViewFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "view_fixture"
version = "0.1.0"
edition = "2021"

//- /src/lib.rs
pub fn answer() -> i32 {
    1
}
"#,
    );

    let target = first_target(&fixture);
    let body_ref = fixture
        .first_body_ref(target)
        .expect("fixture should contain one lowered function body");
    let body = fixture
        .resident_body(body_ref)
        .expect("fixture body should be resident");
    let view_db = fixture.view_db();

    let ty = TyView::new(&view_db)
        .ty_for_expr(ExprRef::new(body_ref, body.root_expr()))?
        .expect("root expression should have an inferred type");

    assert_eq!(ty, Ty::Primitive(PrimitiveTy::SignedInt(SignedIntTy::I32)));
    Ok(())
}

#[test]
fn resolves_body_value_declarations_through_view_fixture() -> anyhow::Result<()> {
    let fixture = ViewFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "view_fixture"
version = "0.1.0"
edition = "2021"

//- /src/lib.rs
const VALUE: i32 = 1;

pub fn read() -> i32 {
    VALUE
}
"#,
    );

    let target = first_target(&fixture);
    let body_ref = fixture
        .body_refs_for_target(target)
        .into_iter()
        .find(|body_ref| {
            matches!(
                fixture.resident_body_owner(*body_ref),
                Some(BodyOwner::Function(_))
            )
        })
        .expect("fixture should contain a lowered function body");
    let value_expr = expr_with_source_text(&fixture, body_ref, "VALUE");
    let view_db = fixture.view_db();

    let declarations = ResolutionView::new(&view_db).declarations_for_expr(value_expr)?;

    assert!(
        declarations
            .iter()
            .any(|declaration| matches!(declaration, DeclarationRef::Item(_))),
        "VALUE expression should resolve to an item declaration: {declarations:?}"
    );
    Ok(())
}

fn first_target(fixture: &ViewFixture) -> TargetRef {
    let (package_idx, package) = fixture
        .parse_db()
        .packages()
        .iter()
        .enumerate()
        .next()
        .expect("fixture should contain a package");
    let target = package
        .targets()
        .iter()
        .next()
        .expect("fixture package should contain a target");
    TargetRef {
        package: PackageSlot(package_idx),
        target: target.id,
    }
}

fn expr_with_source_text(
    fixture: &ViewFixture,
    body_ref: rg_ir_model::BodyRef,
    text: &str,
) -> ExprRef {
    let body = fixture
        .resident_body(body_ref)
        .expect("fixture body should be resident");
    let package = fixture
        .parse_db()
        .package(body_ref.target.package.0)
        .expect("fixture body package should exist");

    for (idx, expr) in body.exprs().iter().enumerate() {
        let Some(file) = package.parsed_file(expr.source.file_id) else {
            continue;
        };
        if file.text_for_span(expr.source.span).as_deref() == Some(text) {
            return ExprRef::new(body_ref, ExprId(idx));
        }
    }

    panic!("fixture body should contain expression `{text}`");
}
