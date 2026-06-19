use super::super::utils;
use crate::{profile::metric, profile_descriptors};
use expect_test::expect;

#[test]
fn expands_local_macro_rules_items() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_user {
    () => {
        pub struct User;
    };
}

make_user!();
"#,
    );
    let target = project.lib("macro_fixture");

    target
        .entry("User")
        .assert_type_exists("macro expansion should add generated structs to the module scope");
}

#[test]
fn generated_impls_keep_generated_source_identity() {
    utils::check_project_def_map(
        r#"
//- /Cargo.toml
[package]
name = "macro_impl_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_user {
    () => {
        pub struct User;

        impl User {
            pub fn new() -> Self {
                User
            }
        }
    };
}

make_user!();
"#,
        expect![[r#"
            package macro_impl_fixture

            macro_impl_fixture [lib]
            crate
            - User : type [pub struct macro_impl_fixture[lib]::crate::User]
            - make_user : macro [macro_definition macro_impl_fixture[lib]::crate::make_user]
            impls
            - impl generated#0:2
        "#]],
    );
}

#[test]
fn resolves_imports_generated_by_macros() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "macro_import_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod source {
    pub struct Thing;
}

macro_rules! import_thing {
    () => {
        pub use source::Thing;
    };
}

import_thing!();
"#,
    );
    let target = project.lib("macro_import_fixture");

    target
        .entry("Thing")
        .assert_type_exists("macro-generated imports should participate in import resolution");
}

#[test]
fn resolves_dollar_crate_in_generated_imports_to_macro_definition_crate() {
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

macro_rules! import_thing {
    () => {
        pub use $crate::source::Thing;
    };
}

pub use import_thing;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
use dep::import_thing;

import_thing!();
"#,
    );
    let target = project.lib("app");

    target
        .entry("Thing")
        .assert_type_exists("$crate in dependency macros should resolve to the defining crate");
}

#[test]
fn generated_macro_definitions_keep_dollar_crate_from_original_macro() {
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

macro_rules! define_inner {
    () => {
        macro_rules! inner {
            () => {
                pub use $crate::source::Thing;
            };
        }
    };
}

pub use define_inner;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
use dep::define_inner;

define_inner!();
inner!();
"#,
    );
    let target = project.lib("app");

    target.entry("Thing").assert_type_exists(
        "generated macro definitions should preserve the original macro's $crate target",
    );
}

#[test]
fn expands_imported_macro_rules_items() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "imported_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod macros {
    macro_rules! make_user {
        () => {
            pub struct User;
        };
    }

    pub(crate) use make_user;
}

use macros::make_user;

make_user!();
"#,
    );
    let target = project.lib("imported_macro_fixture");

    target
        .entry("User")
        .assert_type_exists("imported macro_rules bindings should expand after import resolution");
}

#[test]
fn expands_macros_generated_by_macros() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "nested_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! define_make_user {
    () => {
        macro_rules! make_user {
            () => {
                pub struct User;
            };
        }
    };
}

define_make_user!();
make_user!();
"#,
    );
    let target = project.lib("nested_macro_fixture");

    target.entry("User").assert_type_exists(
        "a generated macro definition should be available to later item-position calls",
    );
}

#[test]
fn stops_recursive_generated_macro_expansion_at_pass_limit() {
    let run = rg_profile::test_support::ProfileTest::start(
        profile_descriptors(),
        "def_map.finalization,def_map.macros",
    );
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "recursive_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! recurse {
    () => {
        recurse!();
    };
}

recurse!();

pub struct After;
"#,
    );
    let snapshot = run.finish();
    let target = project.lib("recursive_macro_fixture");

    target
        .entry("After")
        .assert_type_exists("macro expansion limit should not abort def-map finalization");
    snapshot.assert_gauge_bool_with_message(
        metric::EXPANSION_PASS_LIMIT_REACHED,
        true,
        "recursive macro expansion should mark the pass limit as reached",
    );
    snapshot.assert_counter_with_message(
        metric::EXPANSION_PASSES,
        128,
        "recursive macro expansion should stop at the configured pass limit",
    );
    snapshot.assert_counter_satisfies_with_message(
        metric::MACRO_CALLS_SKIPPED_BY_LIMIT,
        |skipped| skipped > 0,
        "recursive macro expansion should leave some retryable calls skipped by limit",
    );
}

#[test]
fn local_macro_can_shadow_builtin_macro_name() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "builtin_shadow_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! include {
    () => {
        pub struct LocalInclude;
    };
}

include!();
"#,
    );
    let target = project.lib("builtin_shadow_fixture");

    target.entry("LocalInclude").assert_type_exists(
        "resolved local macros should expand even when their name matches a builtin macro",
    );
}

#[test]
fn qualified_macro_can_use_builtin_macro_name() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "qualified_builtin_name_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod macros {
    macro_rules! include {
        () => {
            pub struct QualifiedInclude;
        };
    }

    pub(crate) use include;
}

macros::include!();
"#,
    );
    let target = project.lib("qualified_builtin_name_fixture");

    target.entry("QualifiedInclude").assert_type_exists(
        "qualified user macros should not be classified as builtins by last segment",
    );
}
