use expect_test::expect;

use super::utils::{AnalysisQuery, check_analysis_queries};

#[test]
fn prepares_and_renames_common_symbol_subjects() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_rename"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User {
    na$field_ref$me: Name,
}

pub struct Name;

pub fn hel$fn_ref$per(user: User) -> Name {
    user.na$field_use$me
}

pub fn use_it() {
    let loc$local_ref$al: Us$type_ref$er;
    let _again: User = local;
    let _name = helper(local);
}
"#,
        &[
            AnalysisQuery::prepare_rename("prepare type rename", "type_ref"),
            AnalysisQuery::rename("rename type", "type_ref", "Account"),
            AnalysisQuery::prepare_rename("prepare field rename", "field_use"),
            AnalysisQuery::rename("rename field", "field_use", "label"),
            AnalysisQuery::rename("rename function", "fn_ref", "build"),
            AnalysisQuery::rename("rename local", "local_ref", "selected"),
        ],
        expect![[r#"
            prepare type rename
            - `User` @ src/lib.rs:12:16-12:20

            rename type
            - target `User` @ src/lib.rs:12:16-12:20
            - `User` -> `Account` @ src/lib.rs:1:12-1:16
            - `User` -> `Account` @ src/lib.rs:7:21-7:25
            - `User` -> `Account` @ src/lib.rs:12:16-12:20
            - `User` -> `Account` @ src/lib.rs:13:17-13:21

            prepare field rename
            - `name` @ src/lib.rs:8:10-8:14

            rename field
            - target `name` @ src/lib.rs:8:10-8:14
            - `name` -> `label` @ src/lib.rs:2:5-2:9
            - `name` -> `label` @ src/lib.rs:8:10-8:14

            rename function
            - target `helper` @ src/lib.rs:7:8-7:14
            - `helper` -> `build` @ src/lib.rs:7:8-7:14
            - `helper` -> `build` @ src/lib.rs:14:17-14:23

            rename local
            - target `local` @ src/lib.rs:12:9-12:14
            - `local` -> `selected` @ src/lib.rs:12:9-12:14
            - `local` -> `selected` @ src/lib.rs:13:24-13:29
            - `local` -> `selected` @ src/lib.rs:14:24-14:29
        "#]],
    );
}

#[test]
fn rename_respects_local_binding_shadowing_same_name_function() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_rename_method_receiver_shadow"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn bar(baz: u8) -> u8 {
    fo$fn_ref$o(baz)
}

fn foo(baz: u8) -> u8 {
    let fo$local_ref$o: Option<u8> = Some(baz);
    foo.map(|baba| baba + baba);
    baz
}
"#,
        &[
            AnalysisQuery::rename("rename function", "fn_ref", "qux"),
            AnalysisQuery::rename("rename local", "local_ref", "maybe"),
        ],
        expect![[r#"
            rename function
            - target `foo` @ src/lib.rs:2:5-2:8
            - `foo` -> `qux` @ src/lib.rs:2:5-2:8
            - `foo` -> `qux` @ src/lib.rs:5:4-5:7

            rename local
            - target `foo` @ src/lib.rs:6:9-6:12
            - `foo` -> `maybe` @ src/lib.rs:6:9-6:12
            - `foo` -> `maybe` @ src/lib.rs:7:5-7:8
        "#]],
    );
}

#[test]
fn rejects_unsupported_rename_targets() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_reject_rename"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod us$module_ref$er {
    pub struct User;
}

use user::User as Acc$alias_ref$ount;

pub fn use_it(_account: Account) {}
"#,
        &[
            AnalysisQuery::prepare_rename("reject module rename", "module_ref"),
            AnalysisQuery::prepare_rename("reject alias rename", "alias_ref"),
        ],
        expect![[r#"
            reject module rename
            - <none>

            reject alias rename
            - <none>
        "#]],
    );
}
