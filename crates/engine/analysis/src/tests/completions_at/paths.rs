use expect_test::expect;

use crate::tests::utils::{AnalysisQuery, check_analysis, check_analysis_queries};
#[test]
fn completes_explicit_empty_import_type_expression_argument_and_generic_argument_paths() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_empty_path_completions
use $0; // complete_with_source("empty import path")

pub struct ModuleType;
pub struct Wrapper<T>(T);
pub const MODULE_VALUE: u8 = 1;

pub fn consume(value: u8) {}

pub fn signature<T, const N: usize>(value: $0) {} // complete_with_source("empty type path")

pub fn body(param: u8) {
    let before = param;
    let _: ModuleType = $0; // complete_with_source("empty expression path")
    consume($0); // complete_with_source("empty argument path")
    let _: Wrapper<$0>; // complete_with_source("empty generic argument path")
}
"#,
        expect![[r#"
            // empty import path
            ct MODULE_VALUE
            st ModuleType
            st Wrapper
            fn body
            fn consume
            kw crate
            kw self
            fn signature
            kw super

            // empty type path
            st ModuleType
            tp T
            st Wrapper
            pt bool
            pt char
            kw crate
            kw dyn
            pt f128
            pt f16
            pt f32
            pt f64
            kw fn
            kw for
            pt i128
            pt i16
            pt i32
            pt i64
            pt i8
            pt isize
            kw self
            pt str
            kw super
            pt u128
            pt u16
            pt u32
            pt u64
            pt u8
            pt usize

            // empty expression path
            ct MODULE_VALUE
            st ModuleType
            st Wrapper
            kw async
            vr before
            fn body
            fn consume
            kw crate
            kw false
            kw if
            kw loop
            kw match
            kw move
            vr param
            kw return
            kw self
            fn signature
            kw super
            kw true

            // empty argument path
            ct MODULE_VALUE
            st ModuleType
            st Wrapper
            kw async
            vr before
            fn body
            fn consume
            kw crate
            kw false
            kw if
            kw loop
            kw match
            kw move
            vr param
            kw return
            kw self
            fn signature
            kw super
            kw true

            // empty generic argument path
            st ModuleType
            st Wrapper
            pt bool
            pt char
            kw crate
            kw dyn
            pt f128
            pt f16
            pt f32
            pt f64
            kw fn
            kw for
            pt i128
            pt i16
            pt i32
            pt i64
            pt i8
            pt isize
            kw self
            pt str
            kw super
            pt u128
            pt u16
            pt u32
            pt u64
            pt u8
            pt usize
        "#]],
    );
}

#[test]
fn completes_qualified_module_paths_in_body_contexts() {
    check_analysis_queries(
        r#"
//- /src/lib.rs crate:analysis_path_completions
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
            // type path completions
            ta ApiAlias
            tt ApiNamed
            en ApiState
            st ApiUser
            md api_nested

            // cast type path completions
            ta ApiAlias
            tt ApiNamed
            en ApiState
            st ApiUser
            md api_nested

            // value path completions
            ta ApiAlias
            tt ApiNamed
            en ApiState
            st ApiUser
            sc FLAG
            ct VERSION
            md api_nested
            fn build_user
        "#]],
    );
}

#[test]
fn completes_bare_qualified_paths_in_value_contexts() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_bare_value_path_completions
pub mod api {
    pub fn build_user() {}
}

pub fn make_root() {}

pub fn use_it() {
    let _foo = crate::$0 // complete("bare value path completions")
}
"#,
        expect![[r#"
            // bare value path completions
            md api
            fn make_root
            fn use_it
        "#]],
    );
}

#[test]
fn completes_bare_qualified_paths_in_type_contexts_without_semicolon() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_bare_type_path_completions
pub mod api {
    pub struct User;
}

pub struct RootType;

pub fn use_it() {
    let _foo: crate::$0 // complete("bare type path completions")
}
"#,
        expect![[r#"
            // bare type path completions
            st RootType
            md api
        "#]],
    );
}

#[test]
fn completes_qualified_paths_with_replacement_range() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_path_completion_metadata
pub mod api {
    pub struct User;
}

pub fn use_it() {
    let _: crate::api::Us$0; // complete_verbose("path metadata completions")
}
"#,
        expect![[r#"
            // path metadata completions
            struct User
              detail: struct User
              sort: User|04|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(0) } })
              replace: 79..81
        "#]],
    );
}
