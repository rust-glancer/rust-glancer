use std::fs;

use rg_body_ir::CurrentSourceSelection;
use rg_ir_model::{PrimitiveTy, Span, UnsignedIntTy};
use rg_profile::test_support::ProfileTest;
use rg_std::CancellationToken;

use crate::{Project, SavedFileChange, profile_descriptors, testonly::ProjectSourceFixture};

#[test]
fn initializer_facts_and_previews_follow_edits_and_restart() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:initializers
macro_rules! seed { () => { 2 + 3 }; }
macro_rules! declare { () => { pub const GENERATED: usize = 9 + 9; }; }
declare!();
pub const LIMIT: usize = 2 + 2;
pub static mut COUNTER: usize = 3 * 3;
pub const EXPANDED: usize = seed!();
pub const LABEL: &str = "a  b";
pub fn outer() {
    const NESTED: usize = 7 * 2;
    static LOCAL: usize = 8 + 8;
    let _ = NESTED;
    let _ = LOCAL;
    let _ = GENERATED;
}
pub const fn neighbor() -> u8 { 1 }
pub fn caller() { let value = neighbor(); let _ = value; let _ = LIMIT; }
"#,
    );
    let path = fixture.path("src/lib.rs");
    let original = fs::read_to_string(&path).expect("fixture source");
    let profile = ProfileTest::start(profile_descriptors(), "body_ir.saved,body_ir.const_eval");
    let mut project = fixture.build_project();
    let profile = profile.finish();
    for counter in ["body_ir.saved.owners", "body_ir.const_eval.initializers"] {
        assert_eq!(
            profile.inner().counter(counter).unwrap_or_default(),
            0,
            "{counter}"
        );
    }

    let check = |project: &Project, text: &str, expected: &[(&str, &str)], current: bool| {
        project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(&path)
                .expect("route file")
                .expect("target");
            let target = (context.crate_ref, context.file);
            let analysis = if current {
                let source = snapshot
                    .prepare_current_source(target, text, &CancellationToken::new())
                    .expect("editor source");
                let (analysis, summary) = snapshot
                    .analysis_for_current_source(
                        target,
                        source,
                        CurrentSourceSelection::IntersectingRange(Span {
                            start: 0,
                            end: text.len() as u32,
                        }),
                        |_| Ok(()),
                    )
                    .expect("editor bodies");
                assert!(summary.is_complete(), "{summary:?}");
                analysis
            } else {
                snapshot.analysis()
            };
            for &(marker, expected) in expected.iter().chain(
                [
                    ("EXPANDED:", "pub const EXPANDED: usize = seed!(); // 5"),
                    // Item-position macro bodies are not yet lowered. Their source preview remains
                    // available independently of whether evaluation can produce a value.
                    ("GENERATED;", "pub const GENERATED: usize = 9 + 9"),
                ]
                .iter(),
            ) {
                let offset = text.rfind(marker).expect("hover marker") as u32;
                let hover = analysis
                    .hover(target.0, target.1, offset)
                    .expect("hover query")
                    .expect("hover");
                assert!(
                    hover.blocks.iter().any(|block| block
                        .signature
                        .as_deref()
                        .is_some_and(|signature| signature.trim_end() == expected)),
                    "{marker}: {hover:?}"
                );
            }
            // Saved declaration targets are remapped to editor text by the LSP layer. Here we
            // check their physical positions after publishing, and current body positions below.
            if !current {
                let offset = text.rfind("neighbor()").expect("neighbor call") as u32;
                let targets = analysis
                    .goto_definition(target.0, target.1, offset)
                    .expect("neighbor navigation");
                let start = text.find("fn neighbor()").expect("neighbor declaration") as u32 + 3;
                assert_eq!(targets.len(), 1);
                assert_eq!(
                    targets[0].span,
                    Some(Span {
                        start,
                        end: start + 8
                    })
                );
            }
            let offset = text.find("value;").expect("local use") as u32;
            let targets = analysis
                .goto_definition(target.0, target.1, offset)
                .expect("local navigation");
            let start = text.rfind("value =").expect("local declaration") as u32;
            assert_eq!(targets.len(), 1);
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
                    .expect("local type")
                    .expect("inferred local")
                    .primitive(),
                Some(PrimitiveTy::UnsignedInt(UnsignedIntTy::U8))
            );
        });
    };
    let original_expected = [
        ("LIMIT:", "pub const LIMIT: usize = 2 + 2; // 4"),
        ("COUNTER:", "pub static mut COUNTER: usize = 3 * 3; // 9"),
        ("NESTED;", "const NESTED: usize = 7 * 2; // 14"),
        ("LOCAL;", "static LOCAL: usize = 8 + 8; // 16"),
        ("LABEL:", "pub const LABEL: &str = \"a  b\""),
    ];
    check(&project, &original, &original_expected, false);

    // Editing one function keeps other saved declarations available. Their hover must still
    // demand a saved value when this request has not rebuilt the initializer itself.
    let edited_function = original.replace("let value = neighbor()", "let value = 1 + neighbor()");
    project.inspect(|snapshot| {
        let context = snapshot
            .file_context_for_path(&path)
            .expect("route file")
            .expect("target");
        let target = (context.crate_ref, context.file);
        let offset = edited_function.rfind("LIMIT;").expect("const reference") as u32;
        let source = snapshot
            .prepare_current_source(target, &edited_function, &CancellationToken::new())
            .expect("edited function source");
        let (analysis, summary) = snapshot
            .analysis_for_current_source(
                target,
                source,
                CurrentSourceSelection::AtOffset(offset),
                |_| Ok(()),
            )
            .expect("edited function");
        assert!(summary.is_complete(), "{summary:?}");
        let hover = analysis
            .hover(target.0, target.1, offset)
            .expect("saved const hover")
            .expect("hover");
        assert!(
            hover
                .blocks
                .iter()
                .any(|block| block.signature.as_deref() == Some(original_expected[0].1)),
            "{hover:?}"
        );
    });

    let moved = format!(
        "// moved declarations\n\n{}",
        original.replace("2 + 2", "2  + 2")
    );
    let moved_expected = [
        ("LIMIT:", "pub const LIMIT: usize = 2  + 2; // 4"),
        original_expected[1],
        original_expected[2],
        original_expected[3],
        original_expected[4],
    ];
    let edited = original
        .replace("2 + 2", "{ 10 + 20 }")
        .replace("3 * 3", "8 * 8")
        .replace("7 * 2", "5 * 5")
        .replace("8 + 8", "9 + 9");
    let edited_expected = [
        ("LIMIT:", "pub const LIMIT: usize = { 10 + 20 }; // 30"),
        ("COUNTER:", "pub static mut COUNTER: usize = 8 * 8; // 64"),
        ("NESTED;", "const NESTED: usize = 5 * 5; // 25"),
        ("LOCAL;", "static LOCAL: usize = 9 + 9; // 18"),
        original_expected[4],
    ];
    // A preview-only expression and a preview beyond the display budget must replace the saved
    // numeric result and short text too, including when the declaration keeps its saved identity.
    let preview_only = edited
        .replace("{ 10 + 20 }", "neighbor() as usize")
        .replace("\"a  b\"", &format!("\"{}\"", "long text ".repeat(20)));
    let preview_expected = [
        ("LIMIT:", "pub const LIMIT: usize = neighbor() as usize"),
        edited_expected[1],
        edited_expected[2],
        edited_expected[3],
        ("LABEL:", "pub const LABEL: &str"),
    ];
    for (text, expected, reuses_facts) in [
        (&moved, &moved_expected, true),
        (&edited, &edited_expected, false),
        (&preview_only, &preview_expected, false),
    ] {
        let profile = ProfileTest::start(profile_descriptors(), "body_ir.const_eval");
        check(&project, text, expected, true);
        let evaluations = profile
            .finish()
            .inner()
            .counter("body_ir.const_eval.initializers")
            .unwrap_or_default();
        assert_eq!(evaluations == 0, reuses_facts);

        let profile = ProfileTest::start(
            profile_descriptors(),
            "def_map.construction,body_ir.const_eval",
        );
        fs::write(&path, text).expect("save initializers");
        project
            .apply_change(SavedFileChange::fs_path(&path))
            .expect("publish initializers");
        check(&project, text, expected, false);
        let profile = profile.finish();
        assert_eq!(
            profile
                .inner()
                .counter("def_map.construction.import_resolution.runs")
                .unwrap_or_default(),
            0
        );
        assert_eq!(
            profile
                .inner()
                .counter("body_ir.const_eval.initializers")
                .unwrap_or_default()
                == 0,
            reuses_facts
        );

        drop(project);
        let profile = ProfileTest::start(
            profile_descriptors(),
            "def_map.construction,body_ir.saved,body_ir.const_eval",
        );
        project = fixture.build_project();
        check(&project, text, expected, false);
        let profile = profile.finish();
        for counter in [
            "def_map.construction.import_resolution.runs",
            "body_ir.saved.inference_owners",
            "body_ir.const_eval.initializers",
        ] {
            assert_eq!(
                profile.inner().counter(counter).unwrap_or_default(),
                0,
                "{counter}"
            );
        }
    }

    // Constants and macro definitions in the same file must not tie declarations to an ordinary
    // function's bytes. Indexing this edit must leave initializer evaluation cold.
    let function_edit = preview_only.replace(
        "fn neighbor() -> u8 { 1 }",
        "fn neighbor() -> u8 { let value = 2; value }",
    );
    let profile = ProfileTest::start(
        profile_descriptors(),
        "def_map.construction,body_ir.saved,body_ir.const_eval",
    );
    fs::write(&path, &function_edit).expect("save function body");
    project
        .apply_change(SavedFileChange::fs_path(&path))
        .expect("publish function body");
    let profile = profile.finish();
    for counter in [
        "def_map.construction.import_resolution.runs",
        "body_ir.saved.inference_owners",
        "body_ir.const_eval.initializers",
    ] {
        assert_eq!(
            profile.inner().counter(counter).unwrap_or_default(),
            0,
            "{counter}"
        );
    }
    check(&project, &function_edit, &preview_expected, false);
}

#[test]
fn expressions_in_declared_types_still_invalidate_signatures() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:signature_expressions
pub const ARRAY: [u8; 1 + 2] = [0; 3];
pub fn array() -> [u8; 2 + 2] { [0; 4] }
pub fn caller() { let constant = ARRAY; let returned = array(); let _ = constant; let _ = returned; }
"#,
    );
    let path = fixture.path("src/lib.rs");
    let original = fs::read_to_string(&path).expect("fixture source");
    let mut project = fixture.build_project();
    for (text, expected, changed) in [
        (original.clone(), [3, 4], false),
        (
            original
                .replace("1 + 2", "3 + 4")
                .replace("2 + 2", "4 + 4")
                .replace("[0; 3]", "[0; 7]")
                .replace("[0; 4]", "[0; 8]"),
            [7, 8],
            true,
        ),
    ] {
        let profile = ProfileTest::start(profile_descriptors(), "def_map.construction");
        fs::write(&path, &text).expect("save type expression");
        project
            .apply_change(SavedFileChange::fs_path(&path))
            .expect("publish type expression");
        project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(&path)
                .expect("route file")
                .expect("target");
            for (name, expected) in ["constant", "returned"].into_iter().zip(expected) {
                let offset = text.find(&format!("{name};")).expect("local use") as u32;
                let hover = snapshot
                    .analysis()
                    .hover(context.crate_ref, context.file, offset)
                    .expect("type hover query")
                    .expect("type hover");
                let expected = format!("let {name}: [u8; {expected}]");
                assert!(
                    hover
                        .blocks
                        .iter()
                        .any(|block| block.signature.as_deref() == Some(&expected)),
                    "{name}: {hover:?}"
                );
            }
        });
        let runs = profile
            .finish()
            .inner()
            .counter("def_map.construction.import_resolution.runs")
            .unwrap_or_default();
        assert_eq!(runs > 0, changed);
    }
}
