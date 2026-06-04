use expect_test::{Expect, expect};
use rg_def_map::PackageSlot;
use rg_ir_model::TargetRef;
use rg_package_store::PackageLoader;

use crate::{
    BindingSurface, BodyCursorCandidate, ValueReferenceSource, ValueReferenceSurface,
    testonly::BodyIrFixture,
};

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

#[test]
fn source_scan_includes_record_shorthand_candidates() {
    check_source_candidates(
        "name",
        r#"
//- /Cargo.toml
[package]
name = "body_cursor_record_shorthand"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
struct User {
    name: u8,
    other: u8,
}

pub fn use_it(input: User, name: u8) -> u8 {
    let built = User { name, other: name };
    let User { name, other: extra } = input;
    name + built.name + extra
}
"#,
        expect![[r#"
            binding @ 6:28-6:32
            expr @ 7:37-7:41
            expr @ 9:18-9:22
            expr @ 9:5-9:9
            record_field User::name @ 7:24-7:28
            record_field User::name @ 8:16-8:20
            record_shorthand_binding name @ 8:16-8:20
            record_shorthand_value name for name @ 7:24-7:28
        "#]],
    );
}

#[test]
fn source_scan_uses_name_span_for_record_pattern_shorthand_bindings() {
    check_source_candidates(
        "name",
        r#"
//- /Cargo.toml
[package]
name = "body_cursor_record_pattern_shorthand"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
enum Option<T> {
    Some(T),
    None,
}

struct User {
    name: Option<u8>,
}

fn use_it(by_ref: User, by_mut: User, by_at: User) {
    let User { ref name } = by_ref;
    let User { mut name } = by_mut;
    match by_at {
        User { name: alias @ Some(_) } => alias,
        User { name: None } => None,
    };
    name;
}
"#,
        expect![[r#"
            expr @ 17:5-17:9
            record_field User::name @ 11:20-11:24
            record_field User::name @ 12:20-12:24
            record_field User::name @ 14:16-14:20
            record_field User::name @ 15:16-15:20
            record_shorthand_binding name @ 11:20-11:24
            record_shorthand_binding name @ 12:20-12:24
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
        BodyCursorCandidate::Binding { surface, .. } => match surface {
            BindingSurface::Plain => "binding".to_string(),
            BindingSurface::RecordPatShorthand { key, .. } => {
                format!("record_shorthand_binding {}", key.declaration_label())
            }
        },
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
        BodyCursorCandidate::ValueReference {
            source, surface, ..
        } => match surface {
            ValueReferenceSurface::Plain => match source {
                ValueReferenceSource::Expr(_) => "expr".to_string(),
                ValueReferenceSource::Path(path) => format!("value_path {path}"),
            },
            ValueReferenceSurface::RecordExprShorthand { key, .. } => {
                let value = match source {
                    ValueReferenceSource::Expr(_) => key.declaration_label(),
                    ValueReferenceSource::Path(path) => path.to_string(),
                };
                format!(
                    "record_shorthand_value {value} for {}",
                    key.declaration_label()
                )
            }
        },
    }
}
