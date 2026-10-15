use expect_test::expect;

use crate::tests::utils::check_analysis;

/// Keep the common completion families honest about the source states editors actually send.
///
/// The unfinished cases model a user who has stopped after the identifier prefix, before typing
/// the statement semicolon. The edit-in-place cases retain syntax after the cursor, as happens
/// when a user returns to a complete expression and replaces part of it.
#[test]
fn completes_common_sites_across_realistic_typed_states() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_typed_states"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod api {
    pub struct PathType;
}

pub struct Widget<T> {
    pub named_field: T,
}

impl<T: Default> Widget<T> {
    pub fn new() -> Self {
        Self { named_field: T::default() }
    }

    pub fn named_method(&self) {}
}

pub struct Record {
    pub record_field: u8,
}

use crate::api::PathT$0 as ImportedPathType; // complete_with_source("edited import path").matching("PathType")

pub fn associated_unfinished() {
    let _ = Widget::<u8>::ne$0 // complete_with_source("unfinished associated path").matching("new")
}

pub fn dot_unfinished(widget: Widget<u8>) {
    let _ = widget.na$0 // complete_with_source("unfinished dot member").matching("named_method")
}

pub fn unqualified_unfinished(local_value: u8) {
    let _ = local_v$0 // complete_with_source("unfinished unqualified name").matching("local_value")
}

pub fn qualified_unfinished() {
    let _: crate::api::PathT$0 // complete_with_source("unfinished qualified type").matching("PathType")
}

pub fn record_unfinished() {
    let _ = Record { record_f$0 } // complete_with_source("unfinished record field").matching("record_field")
}

pub fn pattern_unfinished(record: Record) {
    let Record { record_f$0 } = record // complete_with_source("unfinished pattern field").matching("record_field")
}

pub fn associated_edit() {
    let _ = Widget::<u8>::ne$0(); // complete_with_source("edited associated path").matching("new")
}

pub fn dot_edit(widget: Widget<u8>) {
    let _ = widget.na$0(); // complete_with_source("edited dot member").matching("named_method")
}

pub fn unqualified_edit(local_value: u8) {
    let _ = local_v$0 + 1; // complete_with_source("edited unqualified name").matching("local_value")
}

pub fn qualified_edit() {
    let _: crate::api::PathT$0 = todo!(); // complete_with_source("edited qualified type").matching("PathType")
}

pub fn record_edit() {
    let _ = Record { record_f$0: 1 }; // complete_with_source("edited record field").matching("record_field")
}

pub fn pattern_edit(record: Record) {
    let Record { record_f$0: _ } = record; // complete_with_source("edited pattern field").matching("record_field")
}

use crate::api::PathT$0 // complete_with_source("unfinished import path").matching("PathType")
"#,
        expect![[r#"
            struct PathType // edited import path
            fn new // unfinished associated path
            inherent_method named_method // unfinished dot member
            variable local_value // unfinished unqualified name
            struct PathType // unfinished qualified type
            field record_field // unfinished record field
            field record_field // unfinished pattern field
            fn new // edited associated path
            inherent_method named_method // edited dot member
            variable local_value // edited unqualified name
            struct PathType // edited qualified type
            field record_field // edited record field
            field record_field // edited pattern field
            struct PathType // unfinished import path
        "#]],
    );
}

/// Exercise the closed end of every body scanner with the unfinished construct at file EOF.
///
/// A cursor immediately before a closing brace is still inside the body's half-open source span.
/// A cursor after the last typed identifier is instead equal to that span's end, which is a
/// distinct recovery boundary and must not be represented only by otherwise-complete fixtures.
#[test]
fn completes_common_body_sites_at_incomplete_file_end() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_incomplete_file_end"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod associated;
pub mod bare_associated;
pub mod bare_dot;
pub mod bare_path;
pub mod dot;
pub mod empty_pattern;
pub mod empty_record;
pub mod label;
pub mod pattern;
pub mod qualified;
pub mod record;
pub mod unqualified;

pub mod api {
    pub struct PathType;
}

pub struct Widget<T> {
    pub named_field: T,
}

impl<T: Default> Widget<T> {
    pub fn new() -> Self {
        Self { named_field: T::default() }
    }

    pub fn named_method(&self) {}
}

pub struct Record {
    pub record_field: u8,
}

//- /src/associated.rs
pub fn run() {
    let _ = crate::Widget::<u8>::ne$0 // complete_with_source("associated path at EOF").matching("new")
//- /src/bare_associated.rs
pub fn run() {
    let _ = crate::Widget::<u8>::$0 // complete_with_source("empty associated path at EOF").matching("new")
//- /src/dot.rs
pub fn run(widget: crate::Widget<u8>) {
    let _ = widget.na$0 // complete_with_source("dot member at EOF").matching("named_method")
//- /src/bare_dot.rs
pub fn run(widget: crate::Widget<u8>) {
    let _ = widget.$0 // complete_with_source("empty dot member at EOF").matching("named_method")
//- /src/unqualified.rs
pub fn run(local_value: u8) {
    let _ = local_v$0 // complete_with_source("unqualified name at EOF").matching("local_value")
//- /src/qualified.rs
pub fn run() {
    let _: crate::api::PathT$0 // complete_with_source("qualified type at EOF").matching("PathType")
//- /src/bare_path.rs
pub fn run() {
    let _: crate::api::$0 // complete_with_source("empty qualified path at EOF").matching("PathType")
//- /src/record.rs
pub fn run() {
    let _ = crate::Record { record_f$0 // complete_with_source("record field at EOF").matching("record_field")
//- /src/empty_record.rs
pub fn run() {
    let _ = crate::Record { $0 // complete_with_source("empty record field at EOF").matching("record_field")
//- /src/pattern.rs
pub fn run(record: crate::Record) {
    let crate::Record { record_f$0 // complete_with_source("pattern field at EOF").matching("record_field")
//- /src/empty_pattern.rs
pub fn run(record: crate::Record) {
    let crate::Record { $0 // complete_with_source("empty pattern field at EOF").matching("record_field")
//- /src/label.rs
pub fn run() {
    'inner: loop {
        break 'inn$0 // complete_with_source("loop label at EOF").matching("'inner")
"#,
        expect![[r#"
            fn new // associated path at EOF
            fn new // empty associated path at EOF
            inherent_method named_method // dot member at EOF
            inherent_method named_method // empty dot member at EOF
            variable local_value // unqualified name at EOF
            struct PathType // qualified type at EOF
            struct PathType // empty qualified path at EOF
            field record_field // record field at EOF
            field record_field // empty record field at EOF
            field record_field // pattern field at EOF
            field record_field // empty pattern field at EOF
            label 'inner // loop label at EOF
        "#]],
    );
}

/// Associated type bindings are normally completed before the user types `=`.
///
/// Keep the already-complete form as an edit-in-place control, but also exercise a closing `>`
/// immediately after the prefix and the recovery boundary where neither token exists yet.
#[test]
fn completes_associated_type_bindings_before_the_equals_sign() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_associated_binding_typed_states"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod eof;

pub trait Sequence {
    type Item;
    type Index;
}

pub trait Parameterized<Input> {
    type Output;
}

pub struct PositionalType;

pub fn edit_in_place<T: Sequence<It$0 = u8>>() {} // complete_with_source("binding with existing equals").matching("Item")
pub fn before_equals<T: Sequence<It$0>>() {} // complete_with_source("binding before equals").matching("Item")
pub fn positional_argument<T: Parameterized<PositionalT$0>>() {} // complete_with_source("ordinary positional type remains available").matching("PositionalType")
pub fn binding_after_positional<T: Parameterized<u8, Out$0>>() {} // complete_with_source("binding after positional argument").matching("Output")

pub fn body_local() {
    trait LocalSequence {
        type LocalItem;
    }

    fn consume<T: LocalSequence<Loc$0>>() {} // complete_with_source("body-local binding before equals").matching("LocalItem")
}

//- /src/eof.rs
pub fn run<T: crate::Sequence<It$0 // complete_with_source("binding before equals at EOF").matching("Item")
"#,
        expect![[r#"
            type_alias Item // binding with existing equals
            type_alias Item // binding before equals
            struct PositionalType // ordinary positional type remains available
            type_alias Output // binding after positional argument
            type_alias LocalItem // body-local binding before equals
            type_alias Item // binding before equals at EOF
        "#]],
    );
}
