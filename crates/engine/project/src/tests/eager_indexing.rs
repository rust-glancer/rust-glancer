use rg_ir_model::{PrimitiveTy, UnsignedIntTy};

use crate::{PackageResidencyPolicy, Project, testonly::ProjectSourceFixture};

#[test]
fn queries_complete_policy_excluded_targets_in_both_storage_modes() {
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

    for policy in [
        PackageResidencyPolicy::AllResident,
        PackageResidencyPolicy::AllOffloadable,
    ] {
        let mut project = Project::builder(fixture.workspace_metadata())
            .package_residency_policy(policy)
            .build()
            .expect("eager fixture builds");
        let generation = project.generation_id();
        // Completing the secondary target must preserve the already-built primary target.
        for (marker, expected) in [
            ("secondary", UnsignedIntTy::U16),
            ("primary", UnsignedIntTy::U8),
        ] {
            let position = fixture.markers().position(marker);
            let context = project
                .snapshot()
                .file_context_for_path(fixture.path(&position.path))
                .expect("fixture context resolves")
                .expect("fixture target is indexed");
            project
                .ensure_body_analysis(&[context.crate_ref], &rg_std::CancellationToken::new())
                .expect("requested target completes");
            let analysis = project
                .snapshot()
                .analysis_for_crates(&[context.crate_ref], rg_std::CancellationToken::new())
                .expect("target analysis loads");
            let ty = analysis
                .type_at(context.crate_ref, context.file, position.offset)
                .expect("binding type resolves")
                .expect("binding has a type");
            assert_eq!(
                ty.primitive(),
                Some(PrimitiveTy::UnsignedInt(expected)),
                "{policy:?}: {marker}"
            );
        }
        assert_eq!(project.generation_id(), generation);
    }
}
