mod utils;

use expect_test::expect;

#[test]
fn dumps_lib_and_bin_item_trees() {
    utils::check_project_item_tree(
        r#"
//- /Cargo.toml
[package]
name = "moderate_crate"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[[bin]]
name = "moderate_crate"
path = "src/main.rs"

//- /src/lib.rs
pub mod cli;
pub mod model;

//- /src/model.rs
pub struct Model;

impl Model {
    pub fn new() -> Self {
        Self
    }
}

//- /src/cli.rs
pub fn run() {}

//- /src/main.rs
use std::path::PathBuf;
use moderate_crate::cli::run;

fn main() {
    let _path = PathBuf::new();
    run();
}
"#,
        expect![[r#"
            package moderate_crate

            targets
            - moderate_crate [lib] -> lib.rs

            - moderate_crate [bin] -> main.rs

            files
            file cli.rs
            - pub fn run

            file lib.rs
            - pub module cli [out_of_line cli.rs]
            - pub module model [out_of_line model.rs]

            file main.rs
            - use std::path::PathBuf
              - import named std::path::PathBuf
            - use moderate_crate::cli::run
              - import named moderate_crate::cli::run
            - fn main

            file model.rs
            - pub struct Model
            - impl
        "#]],
    );
}

#[test]
fn lowers_distinct_out_of_line_modules_for_lib_and_bin_roots() {
    utils::check_project_item_tree(
        r#"
//- /Cargo.toml
[package]
name = "target_module_fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[[bin]]
name = "target-module-fixture"
path = "src/main.rs"

//- /src/lib.rs
pub mod library;

//- /src/library.rs
pub struct LibraryThing;

//- /src/main.rs
mod cli;

fn main() {}

//- /src/cli.rs
pub struct CliThing;
"#,
        expect![[r#"
            package target_module_fixture

            targets
            - target_module_fixture [lib] -> lib.rs

            - target-module-fixture [bin] -> main.rs

            files
            file cli.rs
            - pub struct CliThing

            file lib.rs
            - pub module library [out_of_line library.rs]

            file library.rs
            - pub struct LibraryThing

            file main.rs
            - module cli [out_of_line cli.rs]
            - fn main
        "#]],
    );
}

#[test]
fn lowers_shared_out_of_line_file_once_for_multiple_target_roots() {
    utils::check_project_item_tree(
        r#"
//- /Cargo.toml
[package]
name = "shared_module_fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[[bin]]
name = "shared-module-fixture"
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
            package shared_module_fixture

            targets
            - shared_module_fixture [lib] -> lib.rs

            - shared-module-fixture [bin] -> main.rs

            files
            file lib.rs
            - pub module shared [out_of_line shared.rs]

            file main.rs
            - module shared [out_of_line shared.rs]
            - fn main

            file shared.rs
            - pub struct Shared
        "#]],
    );
}

#[test]
fn resolves_out_of_line_multi_module_chains() {
    utils::check_project_item_tree(
        r#"
//- /Cargo.toml
[package]
name = "module_chain_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod outer;

//- /src/outer.rs
pub mod inner;
pub struct Outer;

//- /src/outer/inner.rs
pub mod leaf;
pub struct Inner;

//- /src/outer/inner/leaf.rs
pub struct Leaf;
"#,
        expect![[r#"
            package module_chain_fixture

            targets
            - module_chain_fixture [lib] -> lib.rs

            files
            file lib.rs
            - pub module outer [out_of_line outer.rs]

            file leaf.rs
            - pub struct Leaf

            file inner.rs
            - pub module leaf [out_of_line leaf.rs]
            - pub struct Inner

            file outer.rs
            - pub module inner [out_of_line inner.rs]
            - pub struct Outer
        "#]],
    );
}

#[test]
fn resolves_out_of_line_files_inside_inline_modules() {
    utils::check_project_item_tree(
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

            targets
            - nested_module_fixture [lib] -> lib.rs

            files
            file lib.rs
            - pub module outer [inline]
              - pub module child [out_of_line child.rs]
            - pub use outer::child::work
              - import named outer::child::work

            file child.rs
            - pub fn work
        "#]],
    );
}

#[test]
fn resolves_path_attribute_modules() {
    utils::check_project_item_tree(
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

            targets
            - path_attr_fixture [lib] -> lib.rs

            files
            file api_file.rs
            - pub struct Api

            file lib.rs
            - pub module api [out_of_line api_file.rs]
            - pub module outer [inline]
              - pub module implementation [out_of_line implementation.rs]
            - pub use api::Api
              - import named api::Api
            - pub use outer::implementation::work
              - import named outer::implementation::work

            file implementation.rs
            - pub fn work
        "#]],
    );
}

#[test]
fn dumps_import_payloads() {
    utils::check_project_item_tree(
        r#"
//- /Cargo.toml
[package]
name = "import_crate"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod bar {
    pub mod foo {}
}

extern crate self as current;
extern crate self as _;

use bar::foo::{self, self as imported_foo, work as _, *};
use crate::bar::foo::work as run;
use ::bar::foo;
"#,
        expect![[r#"
            package import_crate

            targets
            - import_crate [lib] -> lib.rs

            files
            file lib.rs
            - pub module bar [inline]
              - pub module foo [inline]
            - extern_crate self [self as current]
            - extern_crate self [self as _]
            - use bar::foo::{self, self as imported_foo, work as _, *}
              - import self bar::foo
              - import self bar::foo as imported_foo
              - import named bar::foo::work as _
              - import glob bar::foo
            - use crate::bar::foo::work as run
              - import named crate::bar::foo::work as run
            - use ::bar::foo
              - import named ::bar::foo
        "#]],
    );
}

#[test]
fn dumps_macro_item_trees() {
    utils::check_project_item_tree(
        r#"
//- /Cargo.toml
[package]
name = "complex_crate"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! label_result {
    ($value:expr) => {
        $value
    };
}

pub fn decorate(input: &str) -> &str {
    label_result!(input)
}
"#,
        expect![[r#"
            package complex_crate

            targets
            - complex_crate [lib] -> lib.rs

            files
            file lib.rs
            - macro_definition label_result
            - pub fn decorate
        "#]],
    );
}

#[test]
fn dumps_declaration_payloads() {
    utils::check_project_item_tree_with_declarations(
        r#"
//- /Cargo.toml
[package]
name = "declaration_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User<T>
where
    T: Clone,
{
    pub id: UserId,
    payload: Option<T>,
}

pub enum LoadState<E> {
    Empty,
    Loaded(User),
    Failed { error: E },
}

pub trait Repository<T>: Send
where
    T: Clone,
{
    type Error;
    const KIND: &'static str;
    fn get(&self, id: UserId) -> Result<T, Self::Error>;
}

impl<T> Repository<T> for DbRepository<T>
where
    T: Clone,
{
    type Error = DbError;
    const KIND: &'static str = "db";
    fn get(&self, id: UserId) -> Result<T, DbError> {
        todo!()
    }
}

pub type UserResult<T> = Result<User<T>, DbError>;
pub const DEFAULT_ID: UserId = UserId(0);
pub static mut CACHE_READY: bool = false;
"#,
        expect![[r#"
            package declaration_fixture

            targets
            - declaration_fixture [lib] -> lib.rs

            files
            file lib.rs
            - pub struct User
              - generics <T> where T: Clone
              - pub field id: UserId
              - field payload: Option<T>
            - pub enum LoadState
              - generics <E>
              - variant Empty
              - variant Loaded
                - field #0: User
              - variant Failed
                - field error: E
            - pub trait Repository
              - generics <T> where T: Clone
              - supertraits Send
              - type_alias Error
              - const KIND
                - ty &'static str
              - fn get
                - params (&self, id: UserId)
                - ret Result<T, Self::Error>
            - impl
              - generics <T> where T: Clone
              - trait Repository<T>
              - self DbRepository<T>
              - type_alias Error
                - aliased DbError
              - const KIND
                - ty &'static str
              - fn get
                - params (&self, id: UserId)
                - ret Result<T, DbError>
            - pub type_alias UserResult
              - generics <T>
              - aliased Result<User<T>, DbError>
            - pub const DEFAULT_ID
              - ty UserId
            - pub static CACHE_READY
              - ty bool
        "#]],
    );
}

#[test]
fn dumps_item_spans() {
    utils::check_project_item_tree_with_spans(
        r#"
//- /Cargo.toml
[package]
name = "simple_crate"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn add_two_numbers(left: i32, right: i32) -> i32 {
    left + right
}
"#,
        expect![[r#"
            package simple_crate

            targets
            - simple_crate [lib] -> lib.rs

            files
            file lib.rs
            - pub fn add_two_numbers [lib.rs 1:1-3:2 (0..73)]
        "#]],
    );
}
