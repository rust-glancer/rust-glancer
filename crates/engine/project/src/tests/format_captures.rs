use rg_analysis::{ReferenceLocation, ReferenceQuery};
use rg_ir_model::Span;

use crate::{Project, testonly::ProjectSourceFixture};

#[test]
fn saved_format_captures_participate_in_references_and_rename() {
    let fixture = ProjectSourceFixture::build_with_fake_sysroot(
        r###"
//- /src/lib.rs crate:format_captures
mod escaped;
use std::format_args as arguments;
pub const $declaration$VALUE: usize = 3;
macro_rules! forward { ($value:expr) => { $value }; }
fn only_format() { let _ = format!("{$format$VALUE}"); }
fn control() { let _ = $ordinary$VALUE; let _ = format!("{$control$VALUE}"); }
fn raw() { let _ = format_args!(r##"{$raw$VALUE}"##); }
fn braces() { let _ = format_args!("\u{7b}$braces$VALUE\u{7d}"); }
fn counts() { let _ = format_args!("{0:>$width$VALUE$.$precision$VALUE$}", 7.0); }
fn named() { let _ = format!("{named} {$named_sibling$VALUE}", named = 7); }
fn forwarded() { let _ = forward!(format!("{$forwarded$VALUE}")); }
fn alias() { let _ = arguments!("{$alias$VALUE}"); }
fn assertion() { assert!(true, "{$assertion$VALUE}"); }
//- /src/escaped.rs
use crate::*;
fn escaped() { let _ = format!("{$escaped$\u{56}ALUE}"); }
"###,
    );
    let sites = [
        ("declaration", "VALUE", false),
        ("ordinary", "VALUE", false),
        ("format", "VALUE", true),
        ("control", "VALUE", true),
        ("raw", "VALUE", true),
        ("braces", "VALUE", true),
        ("width", "VALUE", true),
        ("precision", "VALUE", true),
        ("named_sibling", "VALUE", true),
        ("forwarded", "VALUE", true),
        ("alias", "VALUE", true),
        ("assertion", "VALUE", true),
        ("escaped", r"\u{56}ALUE", true),
    ];
    let check = |project: &Project| {
        project.inspect(|snapshot| {
            let mut expected = sites
                .iter()
                .map(|&(marker, spelling, capture)| {
                    let position = fixture.markers().position(marker);
                    let context = snapshot
                        .file_context_for_path(fixture.path(&position.path))
                        .expect("route occurrence")
                        .expect("occurrence target");
                    (
                        ReferenceLocation {
                            crate_ref: context.crate_ref,
                            file_id: context.file,
                            span: Span {
                                start: position.offset,
                                end: position.offset + spelling.len() as u32,
                            },
                        },
                        spelling,
                        capture,
                    )
                })
                .collect::<Vec<_>>();
            expected.sort_by_key(|(location, _, _)| (location.file_id, location.span.start));
            let declaration = fixture.markers().position("declaration");
            let context = snapshot
                .file_context_for_path(fixture.path(&declaration.path))
                .expect("route declaration")
                .expect("declaration target");
            let crates = [context.crate_ref];
            let query = ReferenceQuery::find_references(&crates, true);
            let analysis = snapshot.analysis();

            // Start from the declaration before opening any function body. A cursor query on the
            // capture would already demand that body and would not exercise source selection.
            assert_eq!(
                analysis
                    .references(context.crate_ref, context.file, declaration.offset, query)
                    .expect("references"),
                expected
                    .iter()
                    .map(|(location, _, _)| location.clone())
                    .collect::<Vec<_>>()
            );
            for marker in ["declaration", "escaped"] {
                let position = fixture.markers().position(marker);
                let context = snapshot
                    .file_context_for_path(fixture.path(&position.path))
                    .expect("route rename")
                    .expect("rename target");
                for new_name in ["renamed", "type"] {
                    let rename = analysis
                        .rename(
                            context.crate_ref,
                            context.file,
                            position.offset,
                            new_name,
                            query,
                        )
                        .expect("rename")
                        .expect("capture has one meaning");
                    assert_eq!(rename.edits.len(), expected.len());
                    for (edit, (location, old_text, capture)) in rename.edits.iter().zip(&expected)
                    {
                        assert_eq!((edit.file_id, edit.span), (location.file_id, location.span));
                        assert_eq!(edit.old_text, *old_text);
                        // Raw identifier syntax belongs to Rust tokens, not to format names.
                        let replacement = if new_name == "type" && !capture {
                            "r#type"
                        } else {
                            new_name
                        };
                        assert_eq!(edit.new_text, replacement);
                    }
                }
            }
        });
    };

    let project = fixture.build_project();
    check(&project);
    drop(project);
    check(&fixture.build_project());
}

#[test]
fn rename_requires_format_literals_to_be_forwarded_without_duplication() {
    for (case, expansion, renameable) in [
        ("single use", "format_args!($template);", true),
        (
            "same target",
            "format_args!($template); format_args!($template);",
            false,
        ),
        (
            "shadowed target",
            "format_args!($template); { const VALUE: u8 = 2; format_args!($template); }",
            false,
        ),
        (
            "explicit argument",
            "format_args!($template); format_args!($template, VALUE = 2);",
            false,
        ),
        (
            "nested body",
            "format_args!($template); fn nested() { const VALUE: u8 = 2; format_args!($template); }",
            false,
        ),
        (
            "ordinary string data",
            "format_args!($template); let _ = $template;",
            false,
        ),
        (
            "ordinary string data in a nested body",
            "format_args!($template); fn nested() { let _ = $template; }",
            false,
        ),
    ] {
        let fixture = ProjectSourceFixture::build_with_fake_sysroot(
            &r#"
//- /src/lib.rs crate:shared_capture
const $declaration$VALUE: u8 = 1;
macro_rules! repeat { ($template:literal) => {{ __EXPANSION__ }}; }
fn show() { repeat!("{$capture$VALUE}"); }
"#
            .replace("__EXPANSION__", expansion),
        );
        let project = fixture.build_project();
        project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(fixture.path("src/lib.rs"))
                .expect("route source")
                .expect("source target");
            let crates = [context.crate_ref];
            let query = ReferenceQuery::find_references(&crates, true);
            let analysis = snapshot.analysis();
            let declaration = fixture.markers().position("declaration").offset;
            let capture = fixture.markers().position("capture").offset;
            let references = analysis
                .references(context.crate_ref, context.file, declaration, query)
                .expect("references");
            assert_eq!(
                references
                    .iter()
                    .map(|location| location.span.start)
                    .collect::<Vec<_>>(),
                [declaration, capture],
                "{case}"
            );
            assert_eq!(
                analysis
                    .prepare_rename(context.crate_ref, context.file, capture)
                    .expect("prepare capture rename")
                    .is_some(),
                renameable,
                "{case}"
            );
            for offset in [declaration, capture] {
                let rename = analysis
                    .rename(context.crate_ref, context.file, offset, "RENAMED", query)
                    .expect("rename");
                assert_eq!(rename.is_some(), renameable, "{case}");
                if let Some(rename) = rename {
                    assert_eq!(
                        rename
                            .edits
                            .iter()
                            .map(|edit| edit.span.start)
                            .collect::<Vec<_>>(),
                        [declaration, capture],
                        "{case}"
                    );
                }
            }
        });
    }
}
