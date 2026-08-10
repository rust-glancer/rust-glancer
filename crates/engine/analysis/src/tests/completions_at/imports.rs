use expect_test::expect;

use super::super::utils::{
    AnalysisQuery, check_analysis_queries, check_analysis_queries_with_fake_sysroot,
};
#[test]
fn completes_unqualified_names_with_bounded_auto_imports() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["catalog", "app"]
resolver = "3"

//- /catalog/Cargo.toml
[package]
name = "catalog"
version = "0.1.0"
edition = "2024"

//- /catalog/src/lib.rs
pub mod collections {
    pub struct BTreeMap;
    pub struct HashMap;
}

pub mod left {
    pub struct Widget;
}

pub mod right {
    pub struct Widget;
}

mod internal {
    #[doc(hidden)]
    pub struct Reexported;
}

pub use internal::Reexported as PublicWidget;

pub mod facade {
    pub struct RankedWidget;
}
pub use facade::RankedWidget;

pub struct VisibleTarget;
#[doc(hidden)]
pub use VisibleTarget as SecretWidget;

#[doc(hidden)]
pub struct HiddenWidget;

#[unstable(feature = "internal", issue = "none")]
pub struct UnstableWidget;

#[cfg(any())]
pub struct DisabledWidget;

pub fn make_widget() {}

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
catalog = { path = "../catalog" }

//- /app/src/lib.rs
use catalog::collections::BTreeMap;

fn types() {
    let _: HashM$hash_map$;
    let _: Wid$ambiguous$;
    let _: RankedW$ranked_reexport$;
    let _: PublicW$public_reexport$;
    let _: SecretW$hidden_reexport$;
    let _: HiddenW$hidden_decl$;
    let _: UnstableW$unstable_decl$;
    let _: DisabledW$disabled_decl$;
}

fn values() {
    make_w$value_name$;
}
"#,
        &[
            AnalysisQuery::complete_verbose_with_source("coalesced HashMap import", "hash_map")
                .in_lib("app")
                .matching("HashMap"),
            AnalysisQuery::complete_verbose_with_source("ambiguous imports", "ambiguous")
                .in_lib("app")
                .matching("Widget"),
            AnalysisQuery::complete_verbose_with_source("short re-export path", "ranked_reexport")
                .in_lib("app")
                .matching("RankedWidget"),
            AnalysisQuery::complete_verbose_with_source("public re-export", "public_reexport")
                .in_lib("app")
                .matching("PublicWidget"),
            AnalysisQuery::complete_with_source("hidden re-export", "hidden_reexport")
                .in_lib("app")
                .matching("SecretWidget"),
            AnalysisQuery::complete_with_source("hidden declaration", "hidden_decl")
                .in_lib("app")
                .matching("HiddenWidget"),
            AnalysisQuery::complete_with_source("unstable declaration", "unstable_decl")
                .in_lib("app")
                .matching("UnstableWidget"),
            AnalysisQuery::complete_with_source("cfg-disabled declaration", "disabled_decl")
                .in_lib("app")
                .matching("DisabledWidget"),
            AnalysisQuery::complete_verbose_with_source("value auto-import", "value_name")
                .in_lib("app")
                .matching("make_widget"),
        ],
        expect![[r#"
            coalesced HashMap import
            - struct HashMap
              detail: struct HashMap (use catalog::collections::HashMap)
              sort: 05-auto-import:0003|00|HashMap|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 61..66
              additional: 4..34 => "catalog::collections::{BTreeMap, HashMap}"

            ambiguous imports
            - struct Widget
              detail: struct Widget (use catalog::left::Widget)
              sort: 05-auto-import:0003|00|Widget|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(2) } })
              replace: 79..82
              additional: 35..35 => "\nuse catalog::left::Widget;"
            - struct Widget
              detail: struct Widget (use catalog::right::Widget)
              sort: 05-auto-import:0003|00|Widget|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(3) } })
              replace: 79..82
              additional: 35..35 => "\nuse catalog::right::Widget;"

            short re-export path
            - struct RankedWidget
              detail: struct RankedWidget (use catalog::RankedWidget)
              sort: 05-auto-import:0002|00|RankedWidget|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(5) } })
              replace: 95..102
              additional: 35..35 => "\nuse catalog::RankedWidget;"

            public re-export
            - struct PublicWidget
              detail: struct PublicWidget (use catalog::PublicWidget)
              sort: 05-auto-import:0002|00|PublicWidget|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(4) } })
              replace: 115..122
              additional: 35..35 => "\nuse catalog::PublicWidget;"

            hidden re-export
            - <none>

            hidden declaration
            - <none>

            unstable declaration
            - <none>

            cfg-disabled declaration
            - <none>

            value auto-import
            - fn make_widget
              detail: pub fn make_widget() (use catalog::make_widget)
              sort: 05-auto-import:0002|make_widget|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), id: FunctionId(0) })
              replace: 229..235
              additional: 35..35 => "\nuse catalog::make_widget;"
              snippet: make_widget()$0
        "#]],
    );
}

#[test]
fn completes_body_local_imported_items() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_import_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    mod local {
        pub struct User;
        pub const VALUE: User = missing();
    }

    use local::*;

    let _typed: U$type$;
    let _value = V$value$;
}
"#,
        &[
            AnalysisQuery::complete("body-local imported type completions", "type"),
            AnalysisQuery::complete("body-local imported value completions", "value"),
        ],
        expect![[r#"
            body-local imported type completions
            - struct User
            - module local

            body-local imported value completions
            - struct User
            - const VALUE
            - variable _typed
            - module local
            - fn use_it
        "#]],
    );
}

#[test]
fn completes_unqualified_import_roots_and_external_roots() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_roots"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[[bin]]
name = "analysis-unqualified-roots"
path = "src/main.rs"

//- /src/lib.rs
pub struct Api;

//- /src/main.rs
use analysis_unqualified_roots$use_root$;

fn main() {
    let _ = analysis_unqualified_roots$value_root$;
}
"#,
        &[
            AnalysisQuery::complete("unqualified use root completions", "use_root")
                .in_bin("analysis_unqualified_roots"),
            AnalysisQuery::complete("unqualified value root completions", "value_root")
                .in_bin("analysis_unqualified_roots"),
        ],
        expect![[r#"
            unqualified use root completions
            - module analysis_unqualified_roots
            - fn main

            unqualified value root completions
            - module analysis_unqualified_roots
            - fn main
        "#]],
    );
}

#[test]
fn completes_absolute_keyword_rooted_paths_and_root_keywords() {
    check_analysis_queries_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_rooted_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod parent {
    pub mod sibling {
        pub struct Sibling;
    }

    pub mod child {
        use ::st$absolute_root$;
        use crate::par$crate_root$;
        use super::sib$super_root$;

        pub fn body() {}
    }
}

"#,
        &[
            AnalysisQuery::complete("absolute root completions", "absolute_root")
                .in_lib("analysis_rooted_path_completions"),
            AnalysisQuery::complete("crate root completions", "crate_root")
                .in_lib("analysis_rooted_path_completions"),
            AnalysisQuery::complete("super root completions", "super_root")
                .in_lib("analysis_rooted_path_completions"),
        ],
        expect![[r#"
            absolute root completions
            - module alloc
            - module core
            - module std

            crate root completions
            - module parent

            super root completions
            - module child
            - module sibling
        "#]],
    );
}

#[test]
fn completes_unqualified_prelude_names() {
    check_analysis_queries_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_prelude_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    let _value: Vec$0;
}
"#,
        &[
            AnalysisQuery::complete("unqualified prelude completions", "0")
                .in_lib("analysis_unqualified_prelude_completions"),
        ],
        expect![[r#"
            unqualified prelude completions
            - trait Fn
            - trait FnMut
            - trait FnOnce
            - trait IntoIterator
            - trait Iterator
            - enum Option
            - enum Result
            - struct String
            - struct Vec
            - module alloc
            - module core
            - module std
        "#]],
    );
}

#[test]
fn completes_qualified_paths_in_use_items() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_use_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
use crate::api::Ap$use_path$;

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
"#,
        &[AnalysisQuery::complete("use path completions", "use_path")],
        expect![[r#"
            use path completions
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
fn completes_qualified_paths_inside_braced_use_items() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_braced_use_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
use crate::api::{Ap$use_path$};

pub mod api {
    pub mod api_nested {}
    pub struct ApiUser;
}
"#,
        &[AnalysisQuery::complete(
            "braced use path completions",
            "use_path",
        )],
        expect![[r#"
            braced use path completions
            - struct ApiUser
            - module api_nested
        "#]],
    );
}

#[test]
fn completes_qualified_paths_at_bare_use_path_coloncolon() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_bare_use_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
use crate::api_notify::$0;

pub mod api_notify {
    pub struct Notification;
}
"#,
        &[AnalysisQuery::complete("bare use path completions", "0")],
        expect![[r#"
            bare use path completions
            - struct Notification
        "#]],
    );
}

#[test]
fn completes_qualified_paths_at_incomplete_bare_use_path_coloncolon() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_incomplete_bare_use_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod api_notify {
    pub struct Notification;
}

use crate::api_notify::$0
"#,
        &[AnalysisQuery::complete(
            "incomplete bare use path completions",
            "0",
        )],
        expect![[r#"
            incomplete bare use path completions
            - struct Notification
        "#]],
    );
}

#[test]
fn completes_sysroot_paths_at_incomplete_bare_use_path_coloncolon() {
    check_analysis_queries_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_sysroot_bare_use_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
use std::sync::$0

#[derive(Debug)]
enum CliInvocation {
    Capture(Vec<String>),
}

const DEFAULT_BASE_BRANCH: &str = "main";

pub fn run() {}
"#,
        &[
            AnalysisQuery::complete("incomplete sysroot use path completions", "0")
                .in_lib("analysis_sysroot_bare_use_path_completions"),
        ],
        expect![[r#"
            incomplete sysroot use path completions
            - struct Arc
        "#]],
    );
}
