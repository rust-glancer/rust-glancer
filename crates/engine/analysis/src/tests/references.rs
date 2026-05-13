use expect_test::expect;

use super::utils::{AnalysisQuery, ReferenceQuery, check_analysis_queries};

#[test]
fn finds_common_reference_subjects() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User {
    na$field_ref$me: Name,
}

pub struct Name;

pub fn helper(user: User) -> Name {
    user.name
}

pub fn use_it() {
    let loc$local_ref$al: Us$type_ref$er;
    let _again: User = local;
    let _name = hel$fn_ref$per(local);
}
"#,
        &[
            AnalysisQuery::references("type references", "type_ref", ReferenceQuery::all()),
            AnalysisQuery::references(
                "type references without declaration",
                "type_ref",
                ReferenceQuery::all().without_declaration(),
            ),
            AnalysisQuery::references("field references", "field_ref", ReferenceQuery::all()),
            AnalysisQuery::references("function references", "fn_ref", ReferenceQuery::all()),
            AnalysisQuery::references("local references", "local_ref", ReferenceQuery::all()),
        ],
        expect![[r#"
            type references
            - `User` @ src/lib.rs:1:12-1:16
            - `User` @ src/lib.rs:7:21-7:25
            - `User` @ src/lib.rs:12:16-12:20
            - `User` @ src/lib.rs:13:17-13:21

            type references without declaration
            - `User` @ src/lib.rs:7:21-7:25
            - `User` @ src/lib.rs:12:16-12:20
            - `User` @ src/lib.rs:13:17-13:21

            field references
            - `name` @ src/lib.rs:2:5-2:9
            - `name` @ src/lib.rs:8:10-8:14

            function references
            - `helper` @ src/lib.rs:7:8-7:14
            - `helper` @ src/lib.rs:14:17-14:23

            local references
            - `local` @ src/lib.rs:12:9-12:14
            - `local` @ src/lib.rs:13:24-13:29
            - `local` @ src/lib.rs:14:24-14:29
        "#]],
    );
}

#[test]
fn finds_body_local_method_references() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_method_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Id;

pub fn use_it() {
    struct User;

    impl User {
        fn i$method_ref$d(&self) -> Id {
            Id
        }
    }

    let user: User;
    user.id();
}
"#,
        &[AnalysisQuery::references(
            "body-local method references",
            "method_ref",
            ReferenceQuery::all(),
        )],
        expect![[r#"
            body-local method references
            - `id` @ src/lib.rs:7:12-7:14
            - `id` @ src/lib.rs:13:10-13:12
        "#]],
    );
}

#[test]
fn scoped_references_keep_external_declaration_without_external_uses() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/helper", "crates/app"]
resolver = "3"

//- /crates/helper/Cargo.toml
[package]
name = "helper"
version = "0.1.0"
edition = "2024"

//- /crates/helper/src/lib.rs
pub struct Tool;

pub fn helper_use(tool: Tool) {
    let _again: Tool = tool;
}

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
helper = { path = "../helper" }

//- /crates/app/src/lib.rs
pub fn use_it() {
    let tool: helper::To$scoped_type_ref$ol = todo!();
    let _again: helper::Tool = tool;
}
"#,
        &[AnalysisQuery::references(
            "scoped type references",
            "scoped_type_ref",
            ReferenceQuery::current_target(),
        )
        .in_lib("app")],
        expect![[r#"
            scoped type references
            - `Tool` @ app/src/lib.rs:2:23-2:27
            - `Tool` @ app/src/lib.rs:3:25-3:29
            - `Tool` @ helper/src/lib.rs:1:12-1:16
        "#]],
    );
}

#[test]
fn file_scoped_references_skip_other_files_in_same_target() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_file_scoped_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod other;

pub struct User;

pub fn use_here() {
    let _: Us$file_scoped_type_ref$er;
}

//- /src/other.rs
use crate::User;

pub fn use_there(_: User) {}
"#,
        &[AnalysisQuery::references(
            "file-scoped type references",
            "file_scoped_type_ref",
            ReferenceQuery::current_file(),
        )],
        expect![[r#"
            file-scoped type references
            - `User` @ src/lib.rs:3:12-3:16
            - `User` @ src/lib.rs:6:12-6:16
        "#]],
    );
}

#[test]
fn scoped_references_from_dependency_include_reverse_dependency_uses() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/app"]
exclude = ["dep", "helper"]
resolver = "3"

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct A$dep_api$pi;

pub fn dep_use(_: Api) {}

//- /helper/Cargo.toml
[package]
name = "helper"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /helper/src/lib.rs
pub fn helper_use(_: dep::Api) {}

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../../dep" }
helper = { path = "../../helper" }

//- /crates/app/src/lib.rs
pub fn app_use(_: dep::Api) {
    helper::helper_use(todo!());
}
"#,
        &[AnalysisQuery::references(
            "dependency-scoped type references",
            "dep_api",
            ReferenceQuery::libs(&["dep", "helper", "app"]),
        )
        .in_lib("dep")],
        expect![[r#"
            dependency-scoped type references
            - `Api` @ app/src/lib.rs:1:24-1:27
            - `Api` @ dep/src/lib.rs:1:12-1:15
            - `Api` @ dep/src/lib.rs:3:19-3:22
            - `Api` @ helper/src/lib.rs:1:27-1:30
        "#]],
    );
}
