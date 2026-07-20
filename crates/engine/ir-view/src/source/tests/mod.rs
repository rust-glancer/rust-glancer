mod utils;

use self::utils::check_source_occurrences;

#[test]
fn source_scan_exposes_expr_occurrences_for_single_segment_expression_paths() {
    check_source_occurrences(
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
        r#"
            binding @ 6:9-6:12
            expr @ 2:5-2:8
            expr @ 7:5-7:8
            local_def @ 5:4-5:7
        "#,
    );
}

#[test]
fn source_scan_exposes_qualified_value_path_prefixes_only_as_type_paths() {
    check_source_occurrences(
        "Action",
        r#"
//- /Cargo.toml
[package]
name = "body_cursor_qualified_value_prefix"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Action {
    Configure,
}

pub fn use_it(action: Action) {
    let _action = Action::Configure;
    match action {
        Action::Configure => {}
    }
}
"#,
        r#"
            local_def @ 1:10-1:16
            type_path Action @ 5:23-5:29
            type_path Action @ 6:19-6:25
            type_path Action @ 8:9-8:15
        "#,
    );
}

#[test]
fn source_scan_exposes_qualified_value_path_final_segments_as_values() {
    check_source_occurrences(
        "Configure",
        r#"
//- /Cargo.toml
[package]
name = "body_cursor_qualified_value_final_segment"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Action {
    Configure,
}

pub fn use_it(action: Action) {
    let _action = Action::Configure;
    match action {
        Action::Configure => {}
    }
}
        "#,
        r#"
            enum_variant @ 2:5-2:14
            value_path Action::Configure @ 6:27-6:36
            value_path Action::Configure @ 8:17-8:26
        "#,
    );
}

#[test]
fn source_scan_includes_explicit_record_field_keys() {
    check_source_occurrences(
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
        r#"
            field @ 3:9-3:13
            record_field User::name @ 7:23-7:27
            record_field User::name @ 8:16-8:20
        "#,
    );
}

#[test]
fn source_scan_includes_record_shorthand_occurrences() {
    check_source_occurrences(
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
        r#"
            binding @ 6:28-6:32
            expr @ 7:37-7:41
            expr @ 9:18-9:22
            expr @ 9:5-9:9
            field @ 2:5-2:9
            record_shorthand_binding name @ 8:16-8:20
            record_shorthand_field User::name @ 7:24-7:28
            record_shorthand_field User::name @ 8:16-8:20
            record_shorthand_value name @ 7:24-7:28
        "#,
    );
}

#[test]
fn source_scan_uses_name_span_for_record_pattern_shorthand_bindings() {
    check_source_occurrences(
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
        r#"
            expr @ 17:5-17:9
            field @ 7:5-7:9
            record_field User::name @ 14:16-14:20
            record_field User::name @ 15:16-15:20
            record_shorthand_binding name @ 11:20-11:24
            record_shorthand_binding name @ 12:20-12:24
            record_shorthand_field User::name @ 11:20-11:24
            record_shorthand_field User::name @ 12:20-12:24
        "#,
    );
}

#[test]
fn source_scan_does_not_expose_generated_body_local_item_type_refs() {
    check_source_occurrences(
        "Text",
        r#"
//- /Cargo.toml
[package]
name = "body_cursor_generated_body_local_item"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Text;

macro_rules! make_item {
    ($ty:ty) => { struct Generated { field: $ty } };
}

pub fn use_it() {
    make_item!(Text);
}
"#,
        r#"
            local_def @ 1:12-1:16
        "#,
    );
}

#[test]
fn source_scan_includes_written_body_macro_calls() {
    check_source_occurrences(
        "make_text",
        r#"
//- /Cargo.toml
[package]
name = "body_cursor_body_macro_calls"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Text;

macro_rules! make_text {
    ($ty:ty) => { let _value: $ty; };
}

pub fn use_it() {
    make_text!(Text);
}
"#,
        r#"
            local_def @ 3:14-3:23
            local_def_reference @ 8:5-8:14
        "#,
    );
}
