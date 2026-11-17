use rg_analysis::{CompletionQuery, CompletionSource};
use rg_source::CapturedSource;
use rg_std::CancellationToken;
use test_fixture::testonly::MarkedText;

use crate::testonly::ProjectSourceFixture;

#[test]
fn edited_declarations_and_incomplete_bodies_use_one_source_view() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:editor_bodies
pub fn inspect() {}
"#,
    );
    let project = fixture.build_project();
    let path = fixture.path("src/lib.rs");
    let generation = project.generation_id();
    let declarations = r#"
pub struct Value { pub field: u8 }
impl Value { pub fn method(&self) {} }
pub trait Factory { fn create() -> Self; }
"#;

    // Every case is a complete replacement of the file. The shared declarations are part of the
    // edit too, so both header lookup and body inference must see newly introduced items.
    for (body, expected) in [
        ("fn inspect(value: Value) { value.met$cursor$; }", "method"),
        (
            "fn inspect(value: Value) { let local = value; local.$cursor$",
            "field",
        ),
        ("fn inspect() { Value { fi$cursor$", "field"),
        (
            "fn inspect() { let value = Value { field: 1 }; let closure = || { value.met$cursor$; }; }",
            "method",
        ),
        (
            "fn inspect() { struct Local; impl Local { fn local_method(&self) {} } fn nested(value: Local) { value.local_$cursor$; } }",
            "local_method",
        ),
        (
            "const VALUE: u8 = { let local = Value { field: 1 }; local.fi$cursor$ };",
            "field",
        ),
        (
            "static VALUE: u8 = { let local = Value { field: 1 }; local.fi$cursor$ };",
            "field",
        ),
        (
            "trait Defaulted { fn run(value: Value) { value.met$cursor$; } }",
            "method",
        ),
        ("fn inspect<Generic>(value: Gene$cursor$) {}", "Generic"),
        ("const VALUE: Val$cursor$ = Value { field: 1 };", "Value"),
        ("impl<Generic> Factory for Gene$cursor$ {}", "Generic"),
        ("impl Factory for Val$cursor$ {}", "Value"),
        ("use crate::Val$cursor$;", "Value"),
    ] {
        let edited = MarkedText::parse(&format!("{declarations}\n{body}"));
        let offset = edited.offset("cursor") as u32;
        let source = CapturedSource::new(&path, edited.text()).expect("editor source");
        project
            .with_editor_snapshot(&[source], CancellationToken::new(), |snapshot| {
                let context = snapshot.file_context_for_path(&path)?.expect("file target");
                let completion =
                    CompletionSource::new(edited.text(), offset).expect("completion syntax");
                let items = snapshot.analysis().completions_at(
                    CompletionQuery::new(context.crate_ref, context.file, offset)
                        .with_completion_source(&completion),
                )?;
                assert!(
                    items.iter().any(|item| item.label == expected),
                    "{body}: {items:?}"
                );
                Ok(())
            })
            .expect("edited completion");
    }
    assert_eq!(project.generation_id(), generation);
}

#[test]
fn edited_impl_members_are_visible_inside_and_outside_the_impl() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:editor_impl
pub struct Service;
impl Service { pub fn existing_static() {} }
"#,
    );
    let project = fixture.build_project();
    let edited = MarkedText::parse(
        r#"
pub struct Service { pub value: u32 }
pub struct AddedType;
impl Service {
    pub fn existing_static(changed: bool) {}
    /// edited constant
    pub const LIMIT: bool = true;
    /// edited associated type
    pub type Output = bool;
    pub fn added_member(&self, other: Service) {
        Self::$associated$;
        self.$member$;
        other.add$parameter$;
        fn nested() { Service::add$nested$; }
        Self::LIM$constant$;
        let _: Self::Out$output$ = false;
        let _: Added$ty$ = loop {};
    }
}
fn outside(value: Service) { value.add$outside$; }
"#,
    );
    let path = fixture.path("src/lib.rs");
    let source = CapturedSource::new(&path, edited.text()).expect("editor source");
    project
        .with_editor_snapshot(&[source], CancellationToken::new(), |snapshot| {
            let context = snapshot.file_context_for_path(&path)?.expect("file target");
            let analysis = snapshot.analysis();
            for (marker, expected, detail, docs) in [
                ("associated", "existing_static", Some("changed: bool"), None),
                ("member", "value", None, None),
                ("parameter", "added_member", Some("other: Service"), None),
                ("nested", "added_member", Some("other: Service"), None),
                ("constant", "LIMIT", None, Some("edited constant")),
                ("output", "Output", None, Some("edited associated type")),
                ("ty", "AddedType", None, None),
                ("outside", "added_member", Some("other: Service"), None),
            ] {
                let offset = edited.offset(marker) as u32;
                let completion =
                    CompletionSource::new(edited.text(), offset).expect("completion syntax");
                let items = analysis.completions_at(
                    CompletionQuery::new(context.crate_ref, context.file, offset)
                        .with_completion_source(&completion),
                )?;
                let rows: Vec<_> = items.iter().filter(|item| item.label == expected).collect();
                assert_eq!(rows.len(), 1, "{marker}: {items:?}");
                if let Some(detail) = detail {
                    assert!(
                        rows[0]
                            .detail
                            .as_deref()
                            .is_some_and(|value| value.contains(detail)),
                        "{rows:?}"
                    );
                }
                if let Some(docs) = docs {
                    assert_eq!(rows[0].documentation.as_deref(), Some(docs));
                }
            }
            Ok(())
        })
        .expect("edited impl completion");
}
