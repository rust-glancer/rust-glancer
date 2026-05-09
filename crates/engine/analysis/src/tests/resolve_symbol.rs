use expect_test::expect;

use super::utils::{AnalysisQuery, check_analysis_queries};

#[test]
fn resolves_body_symbols_to_navigation_targets() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_resolve_symbol"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn helper() -> User {
    User
}

pub fn use_it() {
    let local: User = helper();
    let _again: User = loc$resolve_local$al;
    let _made: User = hel$resolve_item$per();
}
"#,
        &[
            AnalysisQuery::resolve("resolve local", "resolve_local"),
            AnalysisQuery::resolve("resolve item", "resolve_item"),
        ],
        expect![[r#"
            resolve local
            - local local @ 8:9-8:14

            resolve item
            - fn helper @ 3:8-3:14
        "#]],
    );
}

#[test]
fn resolves_body_local_struct_symbols_to_navigation_targets() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_local_struct_resolve"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn make() {
    struct User;
    let _local: Us$resolve_local_type$er = Us$resolve_local_ctor$er;
}
"#,
        &[
            AnalysisQuery::resolve("resolve local type path", "resolve_local_type"),
            AnalysisQuery::resolve("resolve local constructor", "resolve_local_ctor"),
        ],
        expect![[r#"
            resolve local type path
            - struct User @ 4:12-4:16

            resolve local constructor
            - struct User @ 4:12-4:16
        "#]],
    );
}
