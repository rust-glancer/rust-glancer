use expect_test::{Expect, expect};
use rg_def_map::PackageSlot;
use rg_ir_model::TargetRef;
use rg_package_store::PackageLoader;

use crate::{BodyCursorCandidate, testonly::BodyIrFixture};

#[test]
fn source_scan_uses_expr_candidates_for_single_segment_expression_paths() {
    check_source_candidates(
        "foo",
        r#"
//- /Cargo.toml
[package]
name = "body_cursor_single_segment_expr"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn bar(baz: u8) -> u8 {
    foo(baz)
}

fn foo(baz: u8) -> u8 {
    let foo: Option<u8> = Some(baz);
    foo.map(|baba| baba + baba);
    baz
}
"#,
        expect![[r#"
            binding @ 6:9-6:12
            expr @ 2:5-2:8
            expr @ 7:5-7:8
        "#]],
    );
}

#[test]
fn source_scan_includes_explicit_record_field_keys() {
    check_source_candidates(
        "name",
        r#"
//- /Cargo.toml
[package]
name = "body_cursor_record_field_keys"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it(input: u8) -> u8 {
    struct User {
        name: u8,
        other: u8,
    }

    let user = User { name: input, other: input };
    let User { name: extracted, other } = user;
    extracted
}
"#,
        expect![[r#"
            local_field @ 3:9-3:13
            record_field User::name @ 7:23-7:27
            record_field User::name @ 8:16-8:20
        "#]],
    );
}

fn check_source_candidates(ident: &str, fixture: &str, expect: Expect) {
    let db = BodyIrFixture::build(fixture);
    let package = db
        .parse_db()
        .packages()
        .first()
        .expect("fixture should contain one package");
    let target = package
        .targets()
        .first()
        .expect("fixture package should contain one target");
    let file_id = package
        .parsed_files()
        .find(|file| file.path().ends_with("src/lib.rs"))
        .expect("fixture should contain src/lib.rs")
        .file_id();
    let parsed_file = package
        .parsed_file(file_id)
        .expect("fixture source file should be parsed");
    let target = TargetRef {
        package: PackageSlot(0),
        target: target.id,
    };
    let body_ir = db
        .body_ir_db()
        .read_txn(PackageLoader::resident_only("resident body fixture"));

    let mut candidates = Vec::new();
    for candidate in body_ir
        .source_candidates(target, Some(file_id))
        .expect("fixture source candidates should scan")
    {
        let Some(text) = parsed_file.text_for_span(candidate.span()) else {
            continue;
        };
        if text != ident {
            continue;
        }

        candidates.push(format!(
            "{} @ {}",
            render_candidate_kind(&candidate),
            render_candidate_span(
                &candidate,
                parsed_file
                    .line_index()
                    .expect("fixture line index should load")
            )
        ));
    }
    candidates.sort();

    let actual = format!("{}\n", candidates.join("\n"));
    expect.assert_eq(&actual);
}

fn render_candidate_span(
    candidate: &BodyCursorCandidate,
    line_index: &rg_parse::LineIndex,
) -> String {
    let line_column = candidate.span().line_column(line_index);
    format!(
        "{}:{}-{}:{}",
        line_column.start.line + 1,
        line_column.start.column + 1,
        line_column.end.line + 1,
        line_column.end.column + 1
    )
}

fn render_candidate_kind(candidate: &BodyCursorCandidate) -> String {
    match candidate {
        BodyCursorCandidate::Body { .. } => "body".to_string(),
        BodyCursorCandidate::Binding { .. } => "binding".to_string(),
        BodyCursorCandidate::Expr { .. } => "expr".to_string(),
        BodyCursorCandidate::LocalItem { .. } => "local_item".to_string(),
        BodyCursorCandidate::LocalValueItem { .. } => "local_value_item".to_string(),
        BodyCursorCandidate::LocalField { .. } => "local_field".to_string(),
        BodyCursorCandidate::LocalEnumVariant { .. } => "local_variant".to_string(),
        BodyCursorCandidate::LocalFunction { .. } => "local_function".to_string(),
        BodyCursorCandidate::RecordFieldKey { owner, key, .. } => {
            format!("record_field {owner}::{}", key.declaration_label())
        }
        BodyCursorCandidate::TypePath { path, .. } => format!("type_path {path}"),
        BodyCursorCandidate::ValuePath { path, .. } => format!("value_path {path}"),
    }
}
