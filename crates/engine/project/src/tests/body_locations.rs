use std::fs;

use rg_ir_model::{PrimitiveTy, Span, UnsignedIntTy};
use rg_profile::test_support::ProfileTest;
use rg_source::CapturedSource;
use rg_std::CancellationToken;

use crate::{Project, SavedFileChange, profile_descriptors, testonly::ProjectSourceFixture};

#[test]
fn body_trivia_reuses_facts_with_fresh_saved_and_editor_locations() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:body_locations
macro_rules! identity { ($value:expr) => { $value }; }
pub fn outer() {
    const LIMIT: u8 = { let initialized = 1u8; initialized };
    fn nested() -> u8 { let inner = LIMIT; let _ = inner; inner }
    let value = identity!(1u8);
    let _ = value;
    let called = nested();
    let _ = called;
}
pub fn neighbor() { let adjacent = 1u32; let _ = adjacent; }
"#,
    );
    let path = fixture.path("src/lib.rs");
    let original = fs::read_to_string(&path).expect("fixture source");
    let mut project = fixture.build_project();
    let check = |project: &Project, text: &str, edited: bool| {
        let captures = if edited {
            vec![CapturedSource::new(&path, text.to_owned()).expect("editor source")]
        } else {
            Vec::new()
        };
        project
            .with_editor_snapshot(&captures, CancellationToken::new(), |snapshot| {
                let context = snapshot
                    .file_context_for_path(&path)
                    .expect("route file")
                    .expect("target");
                let target = (context.crate_ref, context.file);
                let analysis = snapshot.analysis();
                for (name, expected) in [
                    ("inner", UnsignedIntTy::U8),
                    ("value", UnsignedIntTy::U8),
                    ("called", UnsignedIntTy::U8),
                    ("adjacent", UnsignedIntTy::U32),
                ] {
                    let offset =
                        text.find(&format!("let _ = {name};")).expect("local use") as u32 + 8;
                    assert_eq!(
                        analysis
                            .type_at(target.0, target.1, offset)
                            .expect("local type")
                            .expect("inferred local")
                            .primitive(),
                        Some(PrimitiveTy::UnsignedInt(expected)),
                        "{name}"
                    );
                    let start = text.find(&format!("let {name} =")).expect("binding") as u32 + 4;
                    let targets = analysis
                        .goto_definition(target.0, target.1, offset)
                        .expect("local navigation");
                    assert_eq!(targets.len(), 1, "{name}");
                    assert_eq!(
                        targets[0].span,
                        Some(Span {
                            start,
                            end: start + name.len() as u32
                        }),
                        "{name}"
                    );
                }
                let range = Span::full_file(text.len() as u32);
                let hints = analysis
                    .inlay_hints(target.0, target.1, range, None)
                    .expect("file hints");
                let initialized = text.find("initialized =").expect("initializer binding") as u32;
                assert!(
                    hints
                        .iter()
                        .any(|hint| hint.text_offset() == initialized + 11 && hint.label == ": u8"),
                    "{hints:?}"
                );
                Ok(())
            })
            .expect("query source view");
    };
    check(&project, &original, false);

    let shifted = format!(
        "// moved owners\n\n{}",
        original
            .replace("let _", "/* moved use */ let _")
            .replace("let value", "/* moved binding */ let value")
    );
    // Selecting both roots must preserve the nested body's identities and reuse the saved facts.
    let profile = ProfileTest::start(profile_descriptors(), "body_ir.owner,body_ir.resolution");
    check(&project, &shifted, true);
    assert_eq!(
        profile
            .finish()
            .inner()
            .counter("body_ir.resolution.expression_visits")
            .unwrap_or_default(),
        0
    );

    let profile = ProfileTest::start(profile_descriptors(), "body_ir.owner,body_ir.resolution");
    fs::write(&path, &shifted).expect("save trivia");
    project
        .apply_changes([SavedFileChange::fs_path(&path)])
        .expect("publish trivia");
    check(&project, &shifted, false);
    assert_eq!(
        profile
            .finish()
            .inner()
            .counter("body_ir.owner.inference_owners")
            .unwrap_or_default(),
        0
    );

    drop(project);
    let profile = ProfileTest::start(profile_descriptors(), "body_ir.owner,body_ir.resolution");
    let project = fixture.build_project();
    check(&project, &shifted, false);
    assert_eq!(
        profile
            .finish()
            .inner()
            .counter("body_ir.resolution.expression_visits")
            .unwrap_or_default(),
        0
    );
}

#[test]
fn body_edits_and_semantic_dependencies_refresh_only_demanded_facts() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:body_changes
mod api;
pub fn edited() { let value = 1u8; let _ = value; }
pub fn caller() { let called = api::make(); let _ = called; }
pub fn neighbor() { let adjacent = 1u32; let _ = adjacent; }
//- /src/api.rs
pub fn make() -> u8 { 1 }
"#,
    );
    let path = fixture.path("src/lib.rs");
    let original = fs::read_to_string(&path).expect("fixture source");
    let mut project = fixture.build_project();
    let check = |project: &Project, text: &str, name: &str, expected| {
        project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(&path)
                .expect("route file")
                .expect("target");
            let offset = text.find(&format!("let _ = {name};")).expect("local use") as u32 + 8;
            assert_eq!(
                snapshot
                    .analysis()
                    .type_at(context.crate_ref, context.file, offset)
                    .expect("local type")
                    .expect("inferred local")
                    .primitive(),
                Some(expected)
            );
        });
    };
    check(
        &project,
        &original,
        "value",
        PrimitiveTy::UnsignedInt(UnsignedIntTy::U8),
    );
    check(
        &project,
        &original,
        "called",
        PrimitiveTy::UnsignedInt(UnsignedIntTy::U8),
    );
    check(
        &project,
        &original,
        "adjacent",
        PrimitiveTy::UnsignedInt(UnsignedIntTy::U32),
    );

    let changed = original.replace("value = 1u8", "value = true");
    fs::write(&path, &changed).expect("save expression");
    project
        .apply_changes([SavedFileChange::fs_path(&path)])
        .expect("publish expression");
    let profile = ProfileTest::start(profile_descriptors(), "body_ir.owner");
    check(&project, &changed, "value", PrimitiveTy::Bool);
    check(
        &project,
        &changed,
        "adjacent",
        PrimitiveTy::UnsignedInt(UnsignedIntTy::U32),
    );
    assert_eq!(
        profile
            .finish()
            .inner()
            .counter("body_ir.owner.inference_owners")
            .unwrap_or_default(),
        1
    );

    // The caller's structure is unchanged. Its own inference query must nevertheless observe
    // the changed declaration, even when preparation's output compares equal.
    let api = fixture.path("src/api.rs");
    fs::write(&api, "pub fn make() -> u16 { 1 }\n").expect("save signature");
    project
        .apply_changes([SavedFileChange::fs_path(&api)])
        .expect("publish signature");
    check(
        &project,
        &changed,
        "called",
        PrimitiveTy::UnsignedInt(UnsignedIntTy::U16),
    );
}

#[test]
fn changed_editor_body_infers_its_nested_bodies_together() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:cold_editor_body
pub fn outer() {
    const LIMIT: u8 = 1;
    fn nested() { let nested_value = LIMIT; let _ = nested_value; }
    let value = 1u8;
    let _ = value;
}
"#,
    );
    let path = fixture.path("src/lib.rs");
    let original = fs::read_to_string(&path).expect("fixture source");
    let project = fixture.build_project();
    let changed = original.replace("nested_value = LIMIT", "nested_value = true");
    let profile = ProfileTest::start(profile_descriptors(), "body_ir.owner,body_ir.resolution");
    project
        .with_editor_snapshot(
            &[CapturedSource::new(&path, changed.clone()).expect("editor source")],
            CancellationToken::new(),
            |snapshot| {
                let context = snapshot
                    .file_context_for_path(&path)
                    .expect("route file")
                    .expect("target");
                let target = (context.crate_ref, context.file);
                let offset = changed.find("let _ = value;").expect("local use") as u32 + 8;
                let analysis = snapshot.analysis();
                assert_eq!(
                    analysis
                        .type_at(target.0, target.1, offset)
                        .expect("local type")
                        .expect("inferred local")
                        .primitive(),
                    Some(PrimitiveTy::UnsignedInt(UnsignedIntTy::U8))
                );
                let nested = changed.find("let _ = nested_value;").expect("nested use") as u32 + 8;
                assert_eq!(
                    analysis
                        .type_at(target.0, target.1, nested)
                        .expect("nested type")
                        .expect("inferred nested local")
                        .primitive(),
                    Some(PrimitiveTy::Bool)
                );
                Ok(())
            },
        )
        .expect("query edited source");
    let profile = profile.finish();
    assert_eq!(
        profile
            .inner()
            .counter("body_ir.owner.inference_owners")
            .unwrap_or_default(),
        1
    );
    profile
        .assert_counter_path_satisfies("body_ir.resolution.expression_visits", |visits| visits > 0);
}
