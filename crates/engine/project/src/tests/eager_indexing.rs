use rg_ir_model::{PrimitiveTy, UnsignedIntTy};

use crate::testonly::ProjectSourceFixture;

#[test]
fn prefills_primary_targets_and_demands_secondary_bodies() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "eager_targets"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn primary() { let value = 1u8; let _ = val$primary$ue; }

//- /tests/secondary.rs
#[test]
fn secondary() { let value = 1u16; let _ = val$secondary$ue; }
"#,
    );

    let project = fixture.build_project();
    let generation = project.generation_id();
    assert_eq!(project.stats().prefilled_body_count, 1);

    for (marker, expected) in [
        ("secondary", UnsignedIntTy::U16),
        ("primary", UnsignedIntTy::U8),
    ] {
        let position = fixture.markers().position(marker);
        project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(fixture.path(&position.path))
                .expect("fixture context resolves")
                .expect("fixture target is indexed");
            let ty = snapshot
                .analysis()
                .type_at(context.crate_ref, context.file, position.offset)
                .expect("binding type resolves")
                .expect("binding has a type");
            assert_eq!(
                ty.primitive(),
                Some(PrimitiveTy::UnsignedInt(expected)),
                "{marker}"
            );
        });
    }
    assert_eq!(project.generation_id(), generation);
}
