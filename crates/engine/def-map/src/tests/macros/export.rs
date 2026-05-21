use super::super::utils;

#[test]
fn macro_export_makes_macro_rules_visible_from_crate_root() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub mod source {
    pub struct Thing;
}

mod private_macros {
    #[macro_export]
    macro_rules! export_thing {
        ($name:ident) => {
            pub use $crate::source::Thing as $name;
        };
    }
}

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
use dep::export_thing;

dep::export_thing!(DirectThing);
export_thing!(ImportedThing);
"#,
    );
    let target = project.lib("app");

    target
        .entry("DirectThing")
        .assert_type_exists("qualified #[macro_export] macros should resolve from crate root");
    target
        .entry("ImportedThing")
        .assert_type_exists("imported #[macro_export] macros should behave like root exports");
}

#[test]
fn macro_exported_root_macro_can_be_called_through_crate_path() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "root_macro_export_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
#[macro_export]
macro_rules! exported {
    () => {
        pub struct RootExported;
    };
}

crate::exported!();
"#,
    );
    let target = project.lib("root_macro_export_fixture");

    target.entry("RootExported").assert_type_exists(
        "root #[macro_export] macros should not become ambiguous with their normal binding",
    );
}

#[test]
fn macro_exported_macro_can_be_called_through_crate_path_before_definition() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "early_root_macro_export_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
crate::exported!();

#[macro_export]
macro_rules! exported {
    () => {
        pub struct EarlyExported;
    };
}
"#,
    );
    let target = project.lib("early_root_macro_export_fixture");

    target.entry("EarlyExported").assert_type_exists(
        "#[macro_export] crate-root path bindings should not be filtered by definition order",
    );
}

#[test]
fn cfg_attr_macro_export_makes_macro_rules_visible_from_crate_root() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "cfg_attr_macro_export_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
crate::exported!();

#[cfg_attr(true, macro_export)]
macro_rules! exported {
    () => {
        pub struct CfgAttrExported;
    };
}
"#,
    );
    let target = project.lib("cfg_attr_macro_export_fixture");

    target.entry("CfgAttrExported").assert_type_exists(
        "active cfg_attr macro_export should expose the macro through the crate root",
    );
}

#[test]
fn inactive_cfg_attr_macro_export_does_not_export_macro_rules() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "inactive_cfg_attr_macro_export_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
crate::exported!();

#[cfg_attr(false, macro_export)]
macro_rules! exported {
    () => {
        pub struct HiddenExport;
    };
}
"#,
    );
    let target = project.lib("inactive_cfg_attr_macro_export_fixture");

    target
        .entry("HiddenExport")
        .assert_missing("inactive cfg_attr macro_export should not expose a root macro binding");
}

#[test]
fn non_exported_macro_path_call_before_definition_is_not_visible() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "early_private_macro_path_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
crate::make_hidden!();

macro_rules! make_hidden {
    () => {
        pub struct Hidden;
    };
}
"#,
    );
    let target = project.lib("early_private_macro_path_fixture");

    target
        .entry("Hidden")
        .assert_missing("direct macro_rules path bindings should remain source-order sensitive");
}

#[test]
fn generated_macro_export_makes_macro_visible_from_crate_root() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_macro_export_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! define_exported {
    () => {
        #[cfg_attr(true, macro_export)]
        macro_rules! exported {
            () => {
                pub struct GeneratedExport;
            };
        }
    };
}

mod child {
    define_exported!();
}

exported!();
"#,
    );
    let target = project.lib("generated_macro_export_fixture");

    target.entry("GeneratedExport").assert_type_exists(
        "generated cfg_attr macro_export definitions should be inserted into the crate root",
    );
}
