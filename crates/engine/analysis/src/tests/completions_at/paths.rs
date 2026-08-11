use expect_test::expect;

use super::super::utils::{AnalysisQuery, check_analysis_queries};
#[test]
fn completes_explicit_empty_import_type_expression_argument_and_generic_argument_paths() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_empty_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
use $empty_import$;

pub struct ModuleType;
pub struct Wrapper<T>(T);
pub const MODULE_VALUE: u8 = 1;

pub fn consume(value: u8) {}

pub fn signature<T, const N: usize>(value: $empty_type$) {}

pub fn body(param: u8) {
    let before = param;
    let _: ModuleType = $empty_expression$;
    consume($empty_argument$);
    let _: Wrapper<$empty_generic_argument$>;
}
"#,
        &[
            AnalysisQuery::complete_with_source("empty import path", "empty_import"),
            AnalysisQuery::complete_with_source("empty type path", "empty_type"),
            AnalysisQuery::complete_with_source("empty expression path", "empty_expression"),
            AnalysisQuery::complete_with_source("empty argument path", "empty_argument"),
            AnalysisQuery::complete_with_source(
                "empty generic argument path",
                "empty_generic_argument",
            ),
        ],
        expect![[r#"
            empty import path
            - const MODULE_VALUE
            - struct ModuleType
            - struct Wrapper
            - fn body
            - fn consume
            - keyword crate
            - keyword self
            - fn signature
            - keyword super

            empty type path
            - struct ModuleType
            - type_parameter T
            - struct Wrapper
            - primitive_type bool
            - primitive_type char
            - keyword crate
            - keyword dyn
            - primitive_type f32
            - primitive_type f64
            - keyword fn
            - keyword for
            - primitive_type i128
            - primitive_type i16
            - primitive_type i32
            - primitive_type i64
            - primitive_type i8
            - primitive_type isize
            - keyword self
            - primitive_type str
            - keyword super
            - primitive_type u128
            - primitive_type u16
            - primitive_type u32
            - primitive_type u64
            - primitive_type u8
            - primitive_type usize

            empty expression path
            - const MODULE_VALUE
            - struct ModuleType
            - struct Wrapper
            - keyword async
            - variable before
            - fn body
            - fn consume
            - keyword crate
            - keyword false
            - keyword if
            - keyword loop
            - keyword match
            - keyword move
            - variable param
            - keyword return
            - keyword self
            - fn signature
            - keyword super
            - keyword true

            empty argument path
            - const MODULE_VALUE
            - struct ModuleType
            - struct Wrapper
            - keyword async
            - variable before
            - fn body
            - fn consume
            - keyword crate
            - keyword false
            - keyword if
            - keyword loop
            - keyword match
            - keyword move
            - variable param
            - keyword return
            - keyword self
            - fn signature
            - keyword super
            - keyword true

            empty generic argument path
            - struct ModuleType
            - struct Wrapper
            - primitive_type bool
            - primitive_type char
            - keyword crate
            - keyword dyn
            - primitive_type f32
            - primitive_type f64
            - keyword fn
            - keyword for
            - primitive_type i128
            - primitive_type i16
            - primitive_type i32
            - primitive_type i64
            - primitive_type i8
            - primitive_type isize
            - keyword self
            - primitive_type str
            - keyword super
            - primitive_type u128
            - primitive_type u16
            - primitive_type u32
            - primitive_type u64
            - primitive_type u8
            - primitive_type usize
        "#]],
    );
}

#[test]
fn completes_qualified_module_paths_in_body_contexts() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod api {
    pub mod api_nested {}
    mod private_nested {}

    pub struct ApiUser;
    pub enum ApiState {}
    pub trait ApiNamed {}
    pub type ApiAlias = ApiUser;

    pub const VERSION: u8 = 1;
    pub static FLAG: bool = true;
    pub fn build_user() -> ApiUser {
        ApiUser
    }
}

pub fn use_it() {
    let _: crate::api::Ap$type_path$;
    let _ = 0 as crate::api::Ap$cast_type_path$;
    let _ = crate::api::bu$value_path$();
}
"#,
        &[
            AnalysisQuery::complete("type path completions", "type_path"),
            AnalysisQuery::complete("cast type path completions", "cast_type_path"),
            AnalysisQuery::complete("value path completions", "value_path"),
        ],
        // Value-position paths include type-namespace entries too because modules and nominal
        // types can be intermediate prefixes. Prefix filtering is left to the LSP client.
        expect![[r#"
            type path completions
            - type_alias ApiAlias
            - trait ApiNamed
            - enum ApiState
            - struct ApiUser
            - module api_nested

            cast type path completions
            - type_alias ApiAlias
            - trait ApiNamed
            - enum ApiState
            - struct ApiUser
            - module api_nested

            value path completions
            - type_alias ApiAlias
            - trait ApiNamed
            - enum ApiState
            - struct ApiUser
            - static FLAG
            - const VERSION
            - module api_nested
            - fn build_user
        "#]],
    );
}

#[test]
fn completes_bare_qualified_paths_in_value_contexts() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_bare_value_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod api {
    pub fn build_user() {}
}

pub fn make_root() {}

pub fn use_it() {
    let _foo = crate::$0
}
"#,
        &[AnalysisQuery::complete("bare value path completions", "0")],
        expect![[r#"
            bare value path completions
            - module api
            - fn make_root
            - fn use_it
        "#]],
    );
}

#[test]
fn completes_bare_qualified_paths_in_type_contexts_without_semicolon() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_bare_type_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod api {
    pub struct User;
}

pub struct RootType;

pub fn use_it() {
    let _foo: crate::$0
}
"#,
        &[AnalysisQuery::complete("bare type path completions", "0")],
        expect![[r#"
            bare type path completions
            - struct RootType
            - module api
        "#]],
    );
}

#[test]
fn completes_qualified_paths_with_replacement_range() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_path_completion_metadata"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod api {
    pub struct User;
}

pub fn use_it() {
    let _: crate::api::Us$0;
}
"#,
        &[AnalysisQuery::complete_verbose(
            "path metadata completions",
            "0",
        )],
        expect![[r#"
            path metadata completions
            - struct User
              detail: struct User
              sort: User|04|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(0) } })
              replace: 79..81
        "#]],
    );
}
