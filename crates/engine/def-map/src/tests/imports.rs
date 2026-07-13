use expect_test::expect;

use super::utils;

#[test]
fn explicit_bindings_shadow_globs_independent_of_import_order() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/named-first", "crates/glob-first"]
resolver = "3"

//- /crates/named-first/Cargo.toml
[package]
name = "named-first"
version = "0.1.0"
edition = "2024"

//- /crates/named-first/src/lib.rs
mod explicit;
mod globbed;

use explicit::Thing;
use globbed::*;

//- /crates/named-first/src/explicit.rs
pub struct Thing;

//- /crates/named-first/src/globbed.rs
pub struct Thing;

//- /crates/glob-first/Cargo.toml
[package]
name = "glob-first"
version = "0.1.0"
edition = "2024"

//- /crates/glob-first/src/lib.rs
mod explicit;
mod globbed;

use globbed::*;
use explicit::Thing;

//- /crates/glob-first/src/explicit.rs
pub struct Thing;

//- /crates/glob-first/src/globbed.rs
pub struct Thing;
"#,
    );

    for package in ["named-first", "glob-first"] {
        project.lib(package).entry("Thing").assert_type_source_file(
            "explicit.rs",
            "a named import should replace a glob binding in either source order",
        );
    }
}

#[test]
fn imports_preserve_constructor_namespace_occupancy_and_value_precedence() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "constructor_import_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod named {
    pub struct Unit;
    pub struct Record { pub value: u8 }
    pub struct RestrictedTuple(u8);
}

use named::{
    Record as NamedRecord,
    RestrictedTuple as NamedRestrictedTuple,
    Unit as NamedUnit,
};

mod glob {
    pub struct Tuple(pub u8);
}

use glob::*;

mod shadowed {
    pub struct Choice;
}

use shadowed::*;

#[allow(non_snake_case)]
fn Choice() {}
"#,
    );
    let target = project.lib("constructor_import_fixture");

    target
        .entry("NamedUnit")
        .assert_type_exists("named imports should retain the unit struct type")
        .assert_value_exists("named imports should retain the unit constructor");
    target
        .entry("NamedRecord")
        .assert_type_exists("named imports should retain record struct types")
        .assert_value_missing("record structs should not gain a value binding through imports");
    target
        .entry("NamedRestrictedTuple")
        .assert_type_exists("the visibility of a tuple struct type follows its declaration")
        .assert_value_missing(
            "a tuple constructor should not be imported beyond its positional fields' visibility",
        );
    target
        .entry("Tuple")
        .assert_type_exists("glob imports should retain tuple struct types")
        .assert_value_exists("glob imports should retain tuple constructors");
    target
        .entry("Choice")
        .assert_type_exists("the glob constructor's type slot should remain visible")
        .assert_value_kind(
            crate::LocalDefKind::Function,
            "a direct function should outrank a glob-imported constructor only in value space",
        );
}

#[test]
fn enum_variant_imports_preserve_shape_namespace_occupancy() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "variant_import_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod source {
    pub enum Choice {
        Record { value: u8 },
        Tuple(u8),
        Unit,
    }
}

use source::Choice::{
    Record as NamedRecord,
    Tuple as NamedTuple,
    Unit as NamedUnit,
};
use source::Choice::*;
"#,
    );
    let target = project.lib("variant_import_fixture");

    for name in ["NamedRecord", "Record"] {
        target
            .entry(name)
            .assert_type_exists("record variant imports should occupy the type namespace")
            .assert_value_missing("record variant imports should not create bare values");
    }
    for name in ["NamedTuple", "Tuple", "NamedUnit", "Unit"] {
        target
            .entry(name)
            .assert_type_exists("tuple and unit variant imports should retain their type binding")
            .assert_value_exists(
                "tuple and unit variant imports should retain their value constructor",
            );
    }
}

#[test]
fn direct_bindings_shadow_globs() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "direct_over_glob_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod globbed;

pub struct Thing;
use globbed::*;

//- /src/globbed.rs
pub struct Thing;
"#,
    );

    project
        .lib("direct_over_glob_fixture")
        .entry("Thing")
        .assert_type_source_file(
            "lib.rs",
            "a direct declaration should remain selected over a glob binding",
        );
}

#[test]
fn equal_priority_imports_record_ambiguity() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "ambiguous_import_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod first;
mod second;

use first::Thing;
use second::Thing;

//- /src/first.rs
pub struct Thing;

//- /src/second.rs
pub struct Thing;
"#,
    );

    project
        .lib("ambiguous_import_fixture")
        .entry("Thing")
        .assert_type_ambiguous(
            2,
            "distinct named imports should remain an explicit ambiguity",
        );
}

#[test]
fn duplicate_glob_routes_to_one_definition_are_not_ambiguous() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "duplicate_glob_route_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod source {
    pub struct Thing;
}

mod first {
    pub use crate::source::Thing;
}

mod second {
    pub use crate::source::Thing;
}

use first::*;
use second::*;
"#,
    );

    project
        .lib("duplicate_glob_route_fixture")
        .entry("Thing")
        .assert_type_resolved_with_routes(
            2,
            "two glob routes to one definition should merge under one selected binding",
        );
}

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
fn imports_tuple_and_unit_enum_variants_in_both_namespaces() {
    utils::check_project_def_map(
        r#"
//- /Cargo.toml
[package]
name = "enum_variant_import_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Maybe<T> {
    Some(T),
    None,
}

pub enum Result<T, E> {
    Ok(T),
    Err(E),
}

use Maybe::{Some, None as Nothing};
use Result::*;
"#,
        expect![[r#"
            package enum_variant_import_fixture

            enum_variant_import_fixture [lib]
            crate
            - Err : type [variant enum_variant_import_fixture[lib]::crate::Result::Err] | value [variant enum_variant_import_fixture[lib]::crate::Result::Err]
            - Maybe : type [pub enum enum_variant_import_fixture[lib]::crate::Maybe]
            - Nothing : type [variant enum_variant_import_fixture[lib]::crate::Maybe::None] | value [variant enum_variant_import_fixture[lib]::crate::Maybe::None]
            - Ok : type [variant enum_variant_import_fixture[lib]::crate::Result::Ok] | value [variant enum_variant_import_fixture[lib]::crate::Result::Ok]
            - Result : type [pub enum enum_variant_import_fixture[lib]::crate::Result]
            - Some : type [variant enum_variant_import_fixture[lib]::crate::Maybe::Some] | value [variant enum_variant_import_fixture[lib]::crate::Maybe::Some]
        "#]],
    );
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
