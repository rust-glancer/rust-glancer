use rg_def_map::PackageSlot;
use rg_ir_model::{CrateId, CrateRef};

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
fn identity<T>(value: T) -> T { value }

pub fn use_it() {
    let value: _ = identity(1);
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

    for (_, body) in crate_bodies.body_views() {
        assert_eq!(body.bindings().len(), body.binding_facts().len());
        assert_eq!(body.exprs().len(), body.expr_facts().len());

        // Inference variables belong to the resolution pass. Persisted facts expose only stable
        // semantic types, even when written `_` forced the pass to create a temporary slot.
        assert!(body.binding_facts().iter().all(|facts| !facts.ty.has_var()));
        assert!(body.expr_facts().iter().all(|facts| !facts.ty.has_var()));
    }
}
