use super::super::utils;

#[test]
fn parent_textual_macro_rules_is_visible_in_later_child_module() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "textual_parent_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_user {
    () => {
        pub struct User;
    };
}

mod child {
    make_user!();
}

pub use child::User;
"#,
    );
    let target = project.lib("textual_parent_macro_fixture");

    target
        .entry("User")
        .assert_type_exists("parent textual macro_rules should be visible in later child modules");
}

#[test]
fn parent_textual_macro_rules_is_not_visible_in_earlier_child_module() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "textual_late_parent_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod child {
    make_hidden!();
}

macro_rules! make_hidden {
    () => {
        pub struct Hidden;
    };
}

pub use child::Hidden;
"#,
    );
    let target = project.lib("textual_late_parent_macro_fixture");

    target
        .entry("Hidden")
        .assert_missing("parent textual macro_rules should not be visible before its definition");
}

#[test]
fn same_module_textual_macro_rules_uses_latest_prior_definition() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "textual_macro_shadow_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make {
    () => {
        pub struct A;
    };
}

make!();

macro_rules! make {
    () => {
        pub struct B;
    };
}

make!();
"#,
    );
    let target = project.lib("textual_macro_shadow_fixture");

    target
        .entry("A")
        .assert_type_exists("the first call should use the first textual definition");
    target
        .entry("B")
        .assert_type_exists("the second call should use the later textual definition");
}

#[test]
fn inner_textual_macro_rules_shadows_parent_textual_macro() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "inner_textual_macro_shadow_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make {
    () => {
        pub struct Parent;
    };
}

mod child {
    macro_rules! make {
        () => {
            pub struct Child;
        };
    }

    make!();
}

pub use child::Child;
pub use child::Parent;
"#,
    );
    let target = project.lib("inner_textual_macro_shadow_fixture");

    target
        .entry("Child")
        .assert_type_exists("child textual macro_rules should shadow the parent definition");
    target
        .entry("Parent")
        .assert_missing("the parent textual macro should not be used when child has a match");
}
