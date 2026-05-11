use expect_test::expect;

use super::utils;

#[test]
fn resolves_nested_self_imports_without_binding_literal_self() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "self_import_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod bar {
    pub mod foo {
        pub fn work() {}
    }
}

use bar::foo::{self, self as imported_foo, work};
"#,
    );
    let target = project.lib("self_import_fixture");

    target.entry("foo").assert_module_named(
        "foo",
        "nested self imports should bind the referenced module under its own name",
    );
    target.entry("imported_foo").assert_module_named(
        "foo",
        "aliased nested self imports should keep the referenced module under the alias",
    );
    target
        .entry("work")
        .assert_value_exists("nested self imports should not interfere with sibling imports");
    target
        .entry("self")
        .assert_missing("nested self imports should not leak a literal `self` binding");
}

#[test]
fn ignores_hidden_renames() {
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
pub fn work() {}

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
mod bar {
    pub fn work() {}
}

extern crate dep as _;
use bar::work as _;
"#,
    );
    let target = project.lib("app");

    target
        .entry("bar")
        .assert_type_exists("hidden renames should not remove unrelated local bindings");
    target
        .entry("dep")
        .assert_missing("hidden extern crate renames should not bind the dependency name");
    target
        .entry("work")
        .assert_missing("hidden use renames should not bind the imported item name");
}

#[test]
fn records_unresolved_named_and_glob_imports() {
    utils::check_project_def_map(
        r#"
//- /Cargo.toml
[package]
name = "unresolved_import_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod existing {}

use missing::Thing;
use existing::Missing as Renamed;
pub use existing::missing::*;
"#,
        expect![[r#"
            package unresolved_import_fixture

            unresolved_import_fixture [lib]
            crate
            - existing : type [module unresolved_import_fixture[lib]::crate::existing]
            unresolved imports
            - use missing::Thing
            - use existing::Missing as Renamed
            - pub use existing::missing::*

            crate::existing
        "#]],
    );
}

#[test]
fn records_unresolved_hidden_imports_without_flagging_resolved_hidden_imports() {
    utils::check_project_def_map(
        r#"
//- /Cargo.toml
[package]
name = "hidden_unresolved_import_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod existing {
    pub fn work() {}
}

use existing::work as _;
use missing::Thing as _;
"#,
        expect![[r#"
            package hidden_unresolved_import_fixture

            hidden_unresolved_import_fixture [lib]
            crate
            - existing : type [module hidden_unresolved_import_fixture[lib]::crate::existing]
            unresolved imports
            - use missing::Thing as _

            crate::existing
            - work : value [pub fn hidden_unresolved_import_fixture[lib]::crate::existing::work]
        "#]],
    );
}
