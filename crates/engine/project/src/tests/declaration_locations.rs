use std::fs;

use rg_body_ir::CurrentSourceSelection;
use rg_ir_model::{PrimitiveTy, Span, UnsignedIntTy};
use rg_profile::test_support::ProfileTest;
use rg_std::CancellationToken;

use crate::{Project, SavedFileChange, profile_descriptors, testonly::ProjectSourceFixture};

#[test]
fn declaration_reuse_keeps_current_locations_through_save_and_restart() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:locations
mod model;
pub fn inspect() { let value = model::later(); let _ = value; }
//- /src/model.rs
pub fn first() { let value = 1u8; let _ = value; }
pub fn later() -> u8 { 2 }
"#,
    );
    let path = fixture.path("src/model.rs");
    let original = fs::read_to_string(&path).expect("model source");
    let mut project = fixture.build_project();

    let check = |project: &Project, text: &str, expected| {
        project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(&path)
                .expect("route model")
                .expect("model target");
            let analysis = snapshot.analysis();
            let offset = text.find("value;").expect("local use") as u32;
            let ty = analysis
                .type_at(context.crate_ref, context.file, offset)
                .expect("local type query")
                .expect("local type");
            assert_eq!(ty.primitive(), Some(PrimitiveTy::UnsignedInt(expected)));
            let hover = analysis
                .hover(context.crate_ref, context.file, offset)
                .expect("local hover")
                .expect("hover result");
            assert!(hover.blocks.iter().any(|block| {
                block.signature.as_deref().is_some_and(|signature| {
                    signature.contains(&format!(
                        "value: {}",
                        PrimitiveTy::UnsignedInt(expected).label()
                    ))
                })
            }));

            let caller = snapshot
                .file_context_for_path(fixture.path("src/lib.rs"))
                .expect("route caller")
                .expect("caller target");
            let caller_text =
                fs::read_to_string(fixture.path("src/lib.rs")).expect("caller source");
            let targets = analysis
                .goto_definition(
                    caller.crate_ref,
                    caller.file,
                    caller_text.find("later").expect("call") as u32,
                )
                .expect("navigate to neighbor");
            assert_eq!(targets.len(), 1);
            let start = text.find("later").expect("neighbor name") as u32;
            assert_eq!(targets[0].file_id, context.file);
            assert_eq!(
                targets[0].span,
                Some(Span {
                    start,
                    end: start + 5
                })
            );
        });
    };
    check(&project, &original, UnsignedIntTy::U8);

    // A longer body moves its neighbor, and ordinary expression recovery must not disable reuse.
    for (text, expected) in [
        (format!("// shifted file\n\n{original}"), UnsignedIntTy::U8),
        (
            original.replace("1u8", "{ let longer = 100u128; longer }"),
            UnsignedIntTy::U128,
        ),
        (
            original.replace("let value", "let unfinished = ; let value"),
            UnsignedIntTy::U8,
        ),
    ] {
        // The editor path uses the same declarations with request-local body positions.
        project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(&path)
                .expect("route model")
                .expect("model target");
            let target = (context.crate_ref, context.file);
            let offset = text.find("value;").expect("current local") as u32;
            let source = snapshot
                .prepare_current_source(target, &text, &CancellationToken::new())
                .expect("prepare editor source");
            let (analysis, summary) = snapshot
                .analysis_for_current_source(
                    target,
                    source,
                    CurrentSourceSelection::AtOffset(offset),
                    |_| Ok(()),
                )
                .expect("current body");
            assert!(summary.is_complete(), "{summary:?}");
            let targets = analysis
                .goto_definition(target.0, target.1, offset)
                .expect("current local navigation");
            let start = text.find("value =").expect("current binding") as u32;
            assert_eq!(
                targets[0].span,
                Some(Span {
                    start,
                    end: start + 5
                })
            );
            assert_eq!(
                analysis
                    .type_at(target.0, target.1, offset)
                    .expect("current local type")
                    .expect("inferred local")
                    .primitive(),
                Some(PrimitiveTy::UnsignedInt(expected))
            );
        });

        let profile = ProfileTest::start(profile_descriptors(), "def_map.construction");
        fs::write(&path, &text).expect("save model");
        project
            .apply_change(SavedFileChange::fs_path(&path))
            .expect("publish save");
        check(&project, &text, expected);
        assert_eq!(
            profile
                .finish()
                .inner()
                .counter("def_map.construction.import_resolution.runs")
                .unwrap_or_default(),
            0
        );

        drop(project);
        let profile = ProfileTest::start(profile_descriptors(), "def_map.construction");
        project = fixture.build_project();
        check(&project, &text, expected);
        assert_eq!(
            profile
                .finish()
                .inner()
                .counter("def_map.construction.import_resolution.runs")
                .unwrap_or_default(),
            0
        );
    }

    // A real signature change still invalidates declarations and changes the caller's answer.
    let changed = original.replace("later() -> u8", "later() -> u16");
    fs::write(&path, changed).expect("save signature");
    let profile = ProfileTest::start(profile_descriptors(), "def_map.construction");
    project
        .apply_change(SavedFileChange::fs_path(&path))
        .expect("publish signature");
    project.inspect(|snapshot| {
        let caller = snapshot
            .file_context_for_path(fixture.path("src/lib.rs"))
            .expect("route caller")
            .expect("caller target");
        let text = fs::read_to_string(fixture.path("src/lib.rs")).expect("caller text");
        assert_eq!(
            snapshot
                .analysis()
                .type_at(
                    caller.crate_ref,
                    caller.file,
                    text.find("value;").expect("caller local") as u32
                )
                .expect("caller type")
                .expect("inferred caller")
                .primitive(),
            Some(PrimitiveTy::UnsignedInt(UnsignedIntTy::U16))
        );
    });
    profile
        .finish()
        .assert_counter_path_satisfies("def_map.construction.import_resolution.runs", |runs| {
            runs > 0
        });
}

#[test]
fn macro_provenance_and_derive_locations_survive_source_movement() {
    let fixture = ProjectSourceFixture::build_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:macro_locations
#[macro_use] mod definitions;
mod model;
pub fn inspect() { let generated = model::made(); let _ = generated; }
//- /src/definitions.rs
macro_rules! inner { ($name:ident, $ty:ty) => { pub fn $name() -> $ty { 1 } }; }
macro_rules! outer { ($name:ident, $ty:ty) => { inner!($name, $ty); }; }
macro_rules! wrap { ($name:ident, $ty:ty) => { pub struct $name(pub $ty); }; }
//- /src/model.rs
#[derive(Clone)]
pub struct Model;
pub const LIMIT: u8 = 3;
wrap!(Wrapper, Model);
pub fn first() { let local = 1u8; }
cfg_select! {
    true => { outer!(made, u8); }
}
pub fn cloned(value: Model) { let copy = value.clone(); let _ = copy; }
"#,
    );
    let path = fixture.path("src/model.rs");
    let definitions = fixture.path("src/definitions.rs");
    let original = fs::read_to_string(&path).expect("macro caller");
    let original_definitions = fs::read_to_string(&definitions).expect("macro definitions");
    let mut project = fixture.build_project();
    let check = |project: &Project, text: &str, expected| {
        project.inspect(|snapshot| {
            let caller = snapshot
                .file_context_for_path(fixture.path("src/lib.rs"))
                .expect("route caller")
                .expect("caller target");
            let caller_text = fs::read_to_string(fixture.path("src/lib.rs")).expect("caller text");
            let analysis = snapshot.analysis();
            let ty = analysis
                .type_at(
                    caller.crate_ref,
                    caller.file,
                    caller_text.find("generated;").expect("local use") as u32,
                )
                .expect("generated return type")
                .expect("inferred result");
            assert_eq!(ty.primitive(), Some(PrimitiveTy::UnsignedInt(expected)));
            let targets = analysis
                .goto_definition(
                    caller.crate_ref,
                    caller.file,
                    caller_text.find("made").expect("generated call") as u32,
                )
                .expect("generated navigation");
            assert_eq!(targets.len(), 1);
            let model = snapshot
                .file_context_for_path(&path)
                .expect("route model")
                .expect("model target");
            let type_argument =
                (text.find("Wrapper, Model").expect("tuple field type") + "Wrapper, ".len()) as u32;
            let type_targets = analysis
                .goto_definition(model.crate_ref, model.file, type_argument)
                .expect("generated tuple field type navigation");
            assert_eq!(type_targets.len(), 1);
            let type_start =
                (text.find("struct Model").expect("declared type") + "struct ".len()) as u32;
            assert_eq!(
                type_targets[0].span,
                Some(Span {
                    start: type_start,
                    end: type_start + 5
                })
            );
            let hover = analysis
                .hover(model.crate_ref, model.file, type_argument)
                .expect("generated tuple type hover")
                .expect("type hover");
            assert!(
                hover.blocks.iter().any(|block| block
                    .signature
                    .as_deref()
                    .is_some_and(|signature| signature.contains("pub struct Model"))),
                "{hover:?}"
            );
            let start = text.find("made").expect("macro argument") as u32;
            assert_eq!(targets[0].file_id, model.file);
            assert_eq!(
                targets[0].span,
                Some(Span {
                    start,
                    end: start + 4
                })
            );
            let hover = analysis
                .hover(
                    model.crate_ref,
                    model.file,
                    text.find("copy;").expect("derived result") as u32,
                )
                .expect("derived hover")
                .expect("copy hover");
            assert!(
                hover.blocks.iter().any(|block| block
                    .signature
                    .as_deref()
                    .is_some_and(|signature| signature.contains("copy: Model"))),
                "{hover:?}"
            );
        });
    };
    check(&project, &original, UnsignedIntTy::U8);

    let moved = format!(
        "// moved invocation\n{}",
        original.replace("1u8", "{ let longer = 1u8; longer }")
    );
    let moved_definitions = format!("// moved definitions\n\n{original_definitions}");
    let profile = ProfileTest::start(profile_descriptors(), "def_map.construction");
    fs::write(&path, &moved).expect("move invocation");
    fs::write(&definitions, &moved_definitions).expect("move definitions");
    project
        .apply_changes([
            SavedFileChange::fs_path(&path),
            SavedFileChange::fs_path(&definitions),
        ])
        .expect("publish moved macros");
    check(&project, &moved, UnsignedIntTy::U8);
    assert_eq!(
        profile
            .finish()
            .inner()
            .counter("def_map.construction.import_resolution.runs")
            .unwrap_or_default(),
        0
    );
    drop(project);
    let profile = ProfileTest::start(profile_descriptors(), "def_map.construction");
    project = fixture.build_project();
    check(&project, &moved, UnsignedIntTy::U8);
    assert_eq!(
        profile
            .finish()
            .inner()
            .counter("def_map.construction.import_resolution.runs")
            .unwrap_or_default(),
        0
    );

    // Token edits to either arguments or definitions still rerun the target resolver.
    for (caller, definition, expected) in [
        (
            moved.replace("made, u8", "made, u16"),
            moved_definitions.clone(),
            UnsignedIntTy::U16,
        ),
        (
            moved.clone(),
            moved_definitions.replace("-> $ty", "-> u32"),
            UnsignedIntTy::U32,
        ),
    ] {
        fs::write(&path, &caller).expect("edit argument");
        fs::write(&definitions, definition).expect("edit definition");
        let profile = ProfileTest::start(profile_descriptors(), "def_map.construction");
        project
            .apply_changes([
                SavedFileChange::fs_path(&path),
                SavedFileChange::fs_path(&definitions),
            ])
            .expect("publish macro edits");
        check(&project, &caller, expected);
        profile
            .finish()
            .assert_counter_path_satisfies("def_map.construction.import_resolution.runs", |runs| {
                runs > 0
            });
    }
}
