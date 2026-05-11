use expect_test::expect;

use super::utils;

#[test]
fn dumps_workspace_resolution_flow() {
    utils::check_project_def_map(
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
mod hidden {
    pub struct Thing;
}

pub use hidden::Thing;
pub fn work() {}

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
pub mod nested;

mod source {
    pub fn greet() {}
}

mod middle {
    pub use crate::source::*;
}

mod final_mod {
    pub use crate::middle::*;
}

extern crate dep as dep_alias;

use dep::Thing;
use dep_alias::work as dep_work;
use crate::nested::local_work;
use final_mod::greet;

pub fn make(_: Thing) {
    greet();
    dep_work();
    local_work();
}

//- /crates/app/src/nested.rs
pub fn local_work() {}

//- /crates/app/src/main.rs
use app::make;

fn main() {
    let _ = make;
}
"#,
        expect![[r#"
            package app

            app [lib]
            crate
            - Thing : type [struct dep[lib]::crate::hidden::Thing]
            - dep_alias : type [module dep[lib]::crate]
            - dep_work : value [fn dep[lib]::crate::work]
            - final_mod : type [module app[lib]::crate::final_mod]
            - greet : value [fn app[lib]::crate::source::greet]
            - local_work : value [fn app[lib]::crate::nested::local_work]
            - make : value [pub fn app[lib]::crate::make]
            - middle : type [module app[lib]::crate::middle]
            - nested : type [pub module app[lib]::crate::nested]
            - source : type [module app[lib]::crate::source]

            crate::final_mod
            - greet : value [pub fn app[lib]::crate::source::greet]

            crate::middle
            - greet : value [pub fn app[lib]::crate::source::greet]

            crate::nested
            - local_work : value [pub fn app[lib]::crate::nested::local_work]

            crate::source
            - greet : value [pub fn app[lib]::crate::source::greet]

            app [bin]
            crate
            - main : value [fn app[bin]::crate::main]
            - make : value [fn app[lib]::crate::make]

            package dep

            dep [lib]
            crate
            - Thing : type [pub struct dep[lib]::crate::hidden::Thing]
            - hidden : type [module dep[lib]::crate::hidden]
            - work : value [pub fn dep[lib]::crate::work]

            crate::hidden
            - Thing : type [pub struct dep[lib]::crate::hidden::Thing]
        "#]],
    );
}

#[test]
fn resolves_reexports_from_out_of_line_files_inside_inline_modules() {
    utils::check_project_def_map(
        r#"
//- /Cargo.toml
[package]
name = "nested_module_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod outer {
    pub mod child;
}

pub use outer::child::work;

//- /src/outer/child.rs
pub fn work() {}
"#,
        expect![[r#"
            package nested_module_fixture

            nested_module_fixture [lib]
            crate
            - outer : type [pub module nested_module_fixture[lib]::crate::outer]
            - work : value [pub fn nested_module_fixture[lib]::crate::outer::child::work]

            crate::outer
            - child : type [pub module nested_module_fixture[lib]::crate::outer::child]

            crate::outer::child
            - work : value [pub fn nested_module_fixture[lib]::crate::outer::child::work]
        "#]],
    );
}

#[test]
fn resolves_reexports_from_path_attribute_modules() {
    utils::check_project_def_map(
        r#"
//- /Cargo.toml
[package]
name = "path_attr_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
#[path = "generated/api_file.rs"]
pub mod api;

pub mod outer {
    #[path = "implementation.rs"]
    pub mod implementation;
}

pub use api::Api;
pub use outer::implementation::work;

//- /src/generated/api_file.rs
pub struct Api;

//- /src/outer/implementation.rs
pub fn work() {}
"#,
        expect![[r#"
            package path_attr_fixture

            path_attr_fixture [lib]
            crate
            - Api : type [pub struct path_attr_fixture[lib]::crate::api::Api]
            - api : type [pub module path_attr_fixture[lib]::crate::api]
            - outer : type [pub module path_attr_fixture[lib]::crate::outer]
            - work : value [pub fn path_attr_fixture[lib]::crate::outer::implementation::work]

            crate::api
            - Api : type [pub struct path_attr_fixture[lib]::crate::api::Api]

            crate::outer
            - implementation : type [pub module path_attr_fixture[lib]::crate::outer::implementation]

            crate::outer::implementation
            - work : value [pub fn path_attr_fixture[lib]::crate::outer::implementation::work]
        "#]],
    );
}

#[test]
fn exposes_shared_out_of_line_modules_from_lib_and_bin_roots() {
    utils::check_project_def_map(
        r#"
//- /Cargo.toml
[package]
name = "shared_module_def_map"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[[bin]]
name = "shared-module-def-map"
path = "src/main.rs"

//- /src/lib.rs
pub mod shared;

//- /src/main.rs
mod shared;

fn main() {}

//- /src/shared.rs
pub struct Shared;
"#,
        expect![[r#"
            package shared_module_def_map

            shared_module_def_map [lib]
            crate
            - shared : type [pub module shared_module_def_map[lib]::crate::shared]

            crate::shared
            - Shared : type [pub struct shared_module_def_map[lib]::crate::shared::Shared]

            shared_module_def_map [bin]
            crate
            - main : value [fn shared_module_def_map[bin]::crate::main]
            - shared : type [module shared_module_def_map[bin]::crate::shared]

            crate::shared
            - Shared : type [pub struct shared_module_def_map[bin]::crate::shared::Shared]
        "#]],
    );
}

#[test]
fn records_impl_blocks_without_scope_bindings() {
    utils::check_project_def_map(
        r#"
//- /Cargo.toml
[package]
name = "impl_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Root;

impl Root {}

pub mod nested {
    pub struct Nested;

    impl Nested {}
}
"#,
        expect![[r#"
            package impl_fixture

            impl_fixture [lib]
            crate
            - Root : type [pub struct impl_fixture[lib]::crate::Root]
            - nested : type [pub module impl_fixture[lib]::crate::nested]
            impls
            - impl lib.rs#1

            crate::nested
            - Nested : type [pub struct impl_fixture[lib]::crate::nested::Nested]
            impls
            - impl lib.rs#3
        "#]],
    );
}

#[test]
fn keeps_type_and_value_bindings_separate() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "namespace_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Thing;

#[allow(non_snake_case)]
pub fn Thing() -> Thing {
    Thing
}
"#,
    );

    project
        .lib("namespace_fixture")
        .entry("Thing")
        .assert_type_exists("type namespace should keep the struct")
        .assert_value_exists("value namespace should keep the function");
}
