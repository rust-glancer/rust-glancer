use expect_test::expect;

use super::super::super::utils::{AnalysisQuery, check_analysis_queries};

/// Keep the common completion families honest about the source states editors actually send.
///
/// The unfinished cases model a user who has stopped after the identifier prefix, before typing
/// the statement semicolon. The edit-in-place cases retain syntax after the cursor, as happens
/// when a user returns to a complete expression and replaces part of it.
#[test]
fn completes_common_sites_across_realistic_typed_states() {
    check_analysis_queries(
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

use crate::api::PathT$import_edit$ as ImportedPathType;

pub fn associated_unfinished() {
    let _ = Widget::<u8>::ne$associated_unfinished$
}

pub fn dot_unfinished(widget: Widget<u8>) {
    let _ = widget.na$dot_unfinished$
}

pub fn unqualified_unfinished(local_value: u8) {
    let _ = local_v$unqualified_unfinished$
}

pub fn qualified_unfinished() {
    let _: crate::api::PathT$qualified_unfinished$
}

pub fn record_unfinished() {
    let _ = Record { record_f$record_unfinished$ }
}

pub fn pattern_unfinished(record: Record) {
    let Record { record_f$pattern_unfinished$ } = record
}

pub fn associated_edit() {
    let _ = Widget::<u8>::ne$associated_edit$();
}

pub fn dot_edit(widget: Widget<u8>) {
    let _ = widget.na$dot_edit$();
}

pub fn unqualified_edit(local_value: u8) {
    let _ = local_v$unqualified_edit$ + 1;
}

pub fn qualified_edit() {
    let _: crate::api::PathT$qualified_edit$ = todo!();
}

pub fn record_edit() {
    let _ = Record { record_f$record_edit$: 1 };
}

pub fn pattern_edit(record: Record) {
    let Record { record_f$pattern_edit$: _ } = record;
}

use crate::api::PathT$import_unfinished$
"#,
        &[
            AnalysisQuery::complete_with_source(
                "unfinished associated path",
                "associated_unfinished",
            )
            .matching("new"),
            AnalysisQuery::complete_with_source("unfinished dot member", "dot_unfinished")
                .matching("named_method"),
            AnalysisQuery::complete_with_source(
                "unfinished unqualified name",
                "unqualified_unfinished",
            )
            .matching("local_value"),
            AnalysisQuery::complete_with_source(
                "unfinished qualified type",
                "qualified_unfinished",
            )
            .matching("PathType"),
            AnalysisQuery::complete_with_source("unfinished record field", "record_unfinished")
                .matching("record_field"),
            AnalysisQuery::complete_with_source("unfinished pattern field", "pattern_unfinished")
                .matching("record_field"),
            AnalysisQuery::complete_with_source("unfinished import path", "import_unfinished")
                .matching("PathType"),
            AnalysisQuery::complete_with_source("edited associated path", "associated_edit")
                .matching("new"),
            AnalysisQuery::complete_with_source("edited dot member", "dot_edit")
                .matching("named_method"),
            AnalysisQuery::complete_with_source("edited unqualified name", "unqualified_edit")
                .matching("local_value"),
            AnalysisQuery::complete_with_source("edited qualified type", "qualified_edit")
                .matching("PathType"),
            AnalysisQuery::complete_with_source("edited record field", "record_edit")
                .matching("record_field"),
            AnalysisQuery::complete_with_source("edited pattern field", "pattern_edit")
                .matching("record_field"),
            AnalysisQuery::complete_with_source("edited import path", "import_edit")
                .matching("PathType"),
        ],
        expect![[r#"
            unfinished associated path
            - fn new

            unfinished dot member
            - inherent_method named_method

            unfinished unqualified name
            - variable local_value

            unfinished qualified type
            - struct PathType

            unfinished record field
            - field record_field

            unfinished pattern field
            - field record_field

            unfinished import path
            - struct PathType

            edited associated path
            - fn new

            edited dot member
            - inherent_method named_method

            edited unqualified name
            - variable local_value

            edited qualified type
            - struct PathType

            edited record field
            - field record_field

            edited pattern field
            - field record_field

            edited import path
            - struct PathType
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
    check_analysis_queries(
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
    let _ = crate::Widget::<u8>::ne$associated_eof$
//- /src/bare_associated.rs
pub fn run() {
    let _ = crate::Widget::<u8>::$bare_associated_eof$
//- /src/dot.rs
pub fn run(widget: crate::Widget<u8>) {
    let _ = widget.na$dot_eof$
//- /src/bare_dot.rs
pub fn run(widget: crate::Widget<u8>) {
    let _ = widget.$bare_dot_eof$
//- /src/unqualified.rs
pub fn run(local_value: u8) {
    let _ = local_v$unqualified_eof$
//- /src/qualified.rs
pub fn run() {
    let _: crate::api::PathT$qualified_eof$
//- /src/bare_path.rs
pub fn run() {
    let _: crate::api::$bare_path_eof$
//- /src/record.rs
pub fn run() {
    let _ = crate::Record { record_f$record_eof$
//- /src/empty_record.rs
pub fn run() {
    let _ = crate::Record { $empty_record_eof$
//- /src/pattern.rs
pub fn run(record: crate::Record) {
    let crate::Record { record_f$pattern_eof$
//- /src/empty_pattern.rs
pub fn run(record: crate::Record) {
    let crate::Record { $empty_pattern_eof$
//- /src/label.rs
pub fn run() {
    'inner: loop {
        break 'inn$label_eof$
"#,
        &[
            AnalysisQuery::complete_with_source("associated path at EOF", "associated_eof")
                .matching("new"),
            AnalysisQuery::complete_with_source(
                "empty associated path at EOF",
                "bare_associated_eof",
            )
            .matching("new"),
            AnalysisQuery::complete_with_source("dot member at EOF", "dot_eof")
                .matching("named_method"),
            AnalysisQuery::complete_with_source("empty dot member at EOF", "bare_dot_eof")
                .matching("named_method"),
            AnalysisQuery::complete_with_source("unqualified name at EOF", "unqualified_eof")
                .matching("local_value"),
            AnalysisQuery::complete_with_source("qualified type at EOF", "qualified_eof")
                .matching("PathType"),
            AnalysisQuery::complete_with_source("empty qualified path at EOF", "bare_path_eof")
                .matching("PathType"),
            AnalysisQuery::complete_with_source("record field at EOF", "record_eof")
                .matching("record_field"),
            AnalysisQuery::complete_with_source("empty record field at EOF", "empty_record_eof")
                .matching("record_field"),
            AnalysisQuery::complete_with_source("pattern field at EOF", "pattern_eof")
                .matching("record_field"),
            AnalysisQuery::complete_with_source("empty pattern field at EOF", "empty_pattern_eof")
                .matching("record_field"),
            AnalysisQuery::complete_with_source("loop label at EOF", "label_eof")
                .matching("'inner"),
        ],
        expect![[r#"
            associated path at EOF
            - fn new

            empty associated path at EOF
            - fn new

            dot member at EOF
            - inherent_method named_method

            empty dot member at EOF
            - inherent_method named_method

            unqualified name at EOF
            - variable local_value

            qualified type at EOF
            - struct PathType

            empty qualified path at EOF
            - struct PathType

            record field at EOF
            - field record_field

            empty record field at EOF
            - field record_field

            pattern field at EOF
            - field record_field

            empty pattern field at EOF
            - field record_field

            loop label at EOF
            - label 'inner
        "#]],
    );
}

/// Associated type bindings are normally completed before the user types `=`.
///
/// Keep the already-complete form as an edit-in-place control, but also exercise a closing `>`
/// immediately after the prefix and the recovery boundary where neither token exists yet.
#[test]
fn completes_associated_type_bindings_before_the_equals_sign() {
    check_analysis_queries(
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

pub fn edit_in_place<T: Sequence<It$edit$ = u8>>() {}
pub fn before_equals<T: Sequence<It$before_equals$>>() {}
pub fn positional_argument<T: Parameterized<PositionalT$positional_argument$>>() {}
pub fn binding_after_positional<T: Parameterized<u8, Out$after_positional$>>() {}

pub fn body_local() {
    trait LocalSequence {
        type LocalItem;
    }

    fn consume<T: LocalSequence<Loc$body_local$>>() {}
}

//- /src/eof.rs
pub fn run<T: crate::Sequence<It$eof$
"#,
        &[
            AnalysisQuery::complete_with_source("binding with existing equals", "edit")
                .matching("Item"),
            AnalysisQuery::complete_with_source("binding before equals", "before_equals")
                .matching("Item"),
            AnalysisQuery::complete_with_source(
                "ordinary positional type remains available",
                "positional_argument",
            )
            .matching("PositionalType"),
            AnalysisQuery::complete_with_source(
                "binding after positional argument",
                "after_positional",
            )
            .matching("Output"),
            AnalysisQuery::complete_with_source("body-local binding before equals", "body_local")
                .matching("LocalItem"),
            AnalysisQuery::complete_with_source("binding before equals at EOF", "eof")
                .matching("Item"),
        ],
        expect![[r#"
            binding with existing equals
            - type_alias Item

            binding before equals
            - type_alias Item

            ordinary positional type remains available
            - struct PositionalType

            binding after positional argument
            - type_alias Output

            body-local binding before equals
            - type_alias LocalItem

            binding before equals at EOF
            - type_alias Item
        "#]],
    );
}
