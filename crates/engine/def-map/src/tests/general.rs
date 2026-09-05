use expect_test::expect;

use super::utils;

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
            - Api : type [pub struct path_attr_fixture[lib]::crate::api::Api] | value [pub struct path_attr_fixture[lib]::crate::api::Api]
            - api : type [pub module path_attr_fixture[lib]::crate::api]
            - outer : type [pub module path_attr_fixture[lib]::crate::outer]
            - work : value [pub fn path_attr_fixture[lib]::crate::outer::implementation::work]

            crate::api
            - Api : type [pub struct path_attr_fixture[lib]::crate::api::Api] | value [pub struct path_attr_fixture[lib]::crate::api::Api]

            crate::outer
            - implementation : type [pub module path_attr_fixture[lib]::crate::outer::implementation]

            crate::outer::implementation
            - work : value [pub fn path_attr_fixture[lib]::crate::outer::implementation::work]
        "#]],
    );
}

#[test]
fn module_collection_terminates_on_file_cycles() {
    utils::check_project_def_map(
        r#"
//- /Cargo.toml
[package]
name = "def_map_module_cycle"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod a;

//- /src/a/mod.rs
#[path = "../lib.rs"]
pub mod root_again;
"#,
        expect![[r#"
            package def_map_module_cycle

            def_map_module_cycle [lib]
            crate
            - a : type [pub module def_map_module_cycle[lib]::crate::a]

            crate::a
            - root_again : type [pub module def_map_module_cycle[lib]::crate::a::root_again]

            crate::a::root_again
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
            - Shared : type [pub struct shared_module_def_map[lib]::crate::shared::Shared] | value [pub struct shared_module_def_map[lib]::crate::shared::Shared]

            shared_module_def_map [bin]
            crate
            - main : value [fn shared_module_def_map[bin]::crate::main]
            - shared : type [module shared_module_def_map[bin]::crate::shared]

            crate::shared
            - Shared : type [pub struct shared_module_def_map[bin]::crate::shared::Shared] | value [pub struct shared_module_def_map[bin]::crate::shared::Shared]
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
            - Root : type [pub struct impl_fixture[lib]::crate::Root] | value [pub struct impl_fixture[lib]::crate::Root]
            - nested : type [pub module impl_fixture[lib]::crate::nested]
            impls
            - impl lib.rs#1

            crate::nested
            - Nested : type [pub struct impl_fixture[lib]::crate::nested::Nested] | value [pub struct impl_fixture[lib]::crate::nested::Nested]
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
pub struct Thing {
    field: (),
}

#[allow(non_snake_case)]
pub fn Thing() -> Thing {
    Thing { field: () }
}
"#,
    );

    project
        .lib("namespace_fixture")
        .entry("Thing")
        .assert_type_exists("type namespace should keep the struct")
        .assert_value_exists("value namespace should keep the function");
}

#[test]
fn struct_field_shape_determines_value_namespace_occupancy() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "struct_namespace_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Unit;
pub struct Tuple(pub u8);
pub struct Record { pub value: u8 }
"#,
    );
    let target = project.lib("struct_namespace_fixture");

    target
        .entry("Unit")
        .assert_type_exists("unit structs should retain their type identity")
        .assert_value_exists("unit struct constructors should occupy the value namespace");
    target
        .entry("Tuple")
        .assert_type_exists("tuple structs should retain their type identity")
        .assert_value_exists("tuple struct constructors should occupy the value namespace");
    target
        .entry("Record")
        .assert_type_exists("record structs should occupy the type namespace")
        .assert_value_missing("record structs should not contribute a value constructor binding");
}

#[test]
fn resident_unresolved_import_totals_are_partitioned_by_package_origin() {
    let fixture = crate::testonly::DefMapFixture::build_with_sysroot(
        r#"
//- /Cargo.toml
[workspace]
members = ["app"]
exclude = ["dep"]
resolver = "3"

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /app/src/lib.rs
use missing_workspace::Thing;

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
use missing_dependency::Thing;

//- /sysroot/library/core/src/lib.rs
use missing_sysroot::Thing;

//- /sysroot/library/alloc/src/lib.rs
pub struct Alloc;

//- /sysroot/library/std/src/lib.rs
pub struct Std;

//- /sysroot/library/proc_macro/src/lib.rs
pub struct TokenStream;
"#,
    );
    let stats = fixture.def_map_db().stats(fixture.workspace());

    assert_eq!(stats.unresolved_imports_by_origin.workspace, 1);
    assert_eq!(stats.unresolved_imports_by_origin.dependency, 1);
    assert_eq!(stats.unresolved_imports_by_origin.sysroot, 1);
    assert_eq!(stats.unresolved_import_count, 3);
    assert_eq!(
        stats.unresolved_import_count,
        stats.unresolved_imports_by_origin.total(),
        "origin totals should describe exactly the resident aggregate",
    );
}
