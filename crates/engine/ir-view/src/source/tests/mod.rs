mod utils;

use self::utils::check_source_occurrences;

#[test]
fn source_scan_classifies_body_path_occurrences() {
    check_source_occurrences(
        r#"
//- /Cargo.toml
[package]
name = "source_occurrence_paths"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Action {
    Configure,
}

pub fn use_action(action: Action) {
    let _action = Action::Configure;
    match action {
        Action::Configure => {}
    }
}

pub fn bar(baz: u8) -> u8 {
    foo(baz)
}

fn foo(baz: u8) -> u8 {
    let foo: Option<u8> = Some(baz);
    foo.map(|baba| baba + baba);
    baz
}
"#,
        &[
            (
                "foo",
                r#"
                    binding @ 17:9-17:12
                    expr @ 13:5-13:8
                    expr @ 18:5-18:8
                    local_def @ 16:4-16:7
                "#,
            ),
            (
                "Action",
                r#"
                    local_def @ 1:10-1:16
                    type_path Action @ 5:27-5:33
                    type_path Action @ 6:19-6:25
                    type_path Action @ 8:9-8:15
                "#,
            ),
            (
                "Configure",
                r#"
                    enum_variant @ 2:5-2:14
                    expr @ 6:27-6:36
                    value_path Action::Configure @ 8:17-8:26
                "#,
            ),
        ],
    );
}

#[test]
fn source_scan_classifies_record_field_surfaces() {
    check_source_occurrences(
        r#"
//- /Cargo.toml
[package]
name = "source_occurrence_records"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
enum Option<T> {
    Some(T),
    None,
}

struct User {
    name: u8,
    other: u8,
}

struct OptionalUser {
    name: Option<u8>,
}

fn explicit(input: u8) -> u8 {
    struct LocalUser {
        name: u8,
        other: u8,
    }

    let user = LocalUser { name: input, other: input };
    let LocalUser { name: extracted, other } = user;
    extracted
}

fn shorthand(input: User, name: u8) -> u8 {
    let built = User { name, other: name };
    let User { name, other: extra } = input;
    name + built.name + extra
}

fn pattern_modifiers(by_ref: OptionalUser, by_mut: OptionalUser, by_at: OptionalUser) {
    let OptionalUser { ref name } = by_ref;
    let OptionalUser { mut name } = by_mut;
    match by_at {
        OptionalUser { name: alias @ Some(_) } => alias,
        OptionalUser { name: None } => None,
    };
    name;
}
"#,
        &[(
            "name",
            r#"
                binding @ 26:27-26:31
                expr @ 27:37-27:41
                expr @ 29:18-29:22
                expr @ 29:5-29:9
                expr @ 39:5-39:9
                field @ 12:5-12:9
                field @ 17:9-17:13
                field @ 7:5-7:9
                record_field LocalUser::name @ 21:28-21:32
                record_field LocalUser::name @ 22:21-22:25
                record_field OptionalUser::name @ 36:24-36:28
                record_field OptionalUser::name @ 37:24-37:28
                record_shorthand_binding name @ 28:16-28:20
                record_shorthand_binding name @ 33:28-33:32
                record_shorthand_binding name @ 34:28-34:32
                record_shorthand_field OptionalUser::name @ 33:28-33:32
                record_shorthand_field OptionalUser::name @ 34:28-34:32
                record_shorthand_field User::name @ 27:24-27:28
                record_shorthand_field User::name @ 28:16-28:20
                record_shorthand_value name @ 27:24-27:28
            "#,
        )],
    );
}

#[test]
fn source_scan_distinguishes_written_and_generated_macro_sources() {
    check_source_occurrences(
        r#"
//- /Cargo.toml
[package]
name = "source_occurrence_macros"
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
        &[
            (
                "Text",
                r#"
                    local_def @ 1:12-1:16
                "#,
            ),
            (
                "make_item",
                r#"
                    local_def @ 3:14-3:23
                    local_def_reference @ 8:5-8:14
                "#,
            ),
        ],
    );
}
