use expect_test::expect;

use super::utils::{AnalysisQuery, check_analysis_queries};

#[test]
fn finds_body_symbols_at_offsets() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_symbol_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn helper() -> User {
    User
}

pub fn use_it() {
    let loc$symbol_decl$al: User = helper();
    let _again: User = loc$symbol_local$al;
    let _made: User = hel$symbol_item$per();
}
"#,
        &[
            AnalysisQuery::symbol("symbol at declaration", "symbol_decl"),
            AnalysisQuery::symbol("symbol at local path", "symbol_local"),
            AnalysisQuery::symbol("symbol at item path", "symbol_item"),
        ],
        expect![[r#"
            symbol at declaration
            - binding let local @ 8:9-8:14

            symbol at local path
            - expr path local @ 9:24-9:29

            symbol at item path
            - expr path helper @ 10:23-10:29
        "#]],
    );
}

#[test]
fn finds_item_and_signature_symbols_at_offsets() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_signature_symbol_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Us$symbol_struct$er;

pub trait Na$symbol_trait$med {
    fn describe(&self) -> User;
}

pub fn make(user: Us$symbol_param$er) -> Us$symbol_ret$er {
    user
}
"#,
        &[
            AnalysisQuery::symbol("symbol at struct declaration", "symbol_struct"),
            AnalysisQuery::symbol("symbol at trait declaration", "symbol_trait"),
            AnalysisQuery::symbol("symbol at parameter type", "symbol_param"),
            AnalysisQuery::symbol("symbol at return type", "symbol_ret"),
        ],
        expect![[r#"
            symbol at struct declaration
            - struct User @ 1:12-1:16

            symbol at trait declaration
            - trait Named @ 3:11-3:16

            symbol at parameter type
            - path User @ 7:19-7:23

            symbol at return type
            - path User @ 7:28-7:32
        "#]],
    );
}

#[test]
fn finds_body_local_struct_symbols_at_offsets() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_local_struct_symbol"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn make() {
    struct Us$symbol_local_struct$er;
    let _local: Us$symbol_local_type$er = Us$symbol_local_ctor$er;
}
"#,
        &[
            AnalysisQuery::symbol("symbol at local struct declaration", "symbol_local_struct"),
            AnalysisQuery::symbol("symbol at local type path", "symbol_local_type"),
            AnalysisQuery::symbol("symbol at local constructor", "symbol_local_ctor"),
        ],
        expect![[r#"
            symbol at local struct declaration
            - struct fn analysis_local_struct_symbol[lib]::crate::make::User @ 4:12-4:16

            symbol at local type path
            - body path User @ 5:17-5:21

            symbol at local constructor
            - expr path User @ 5:24-5:28
        "#]],
    );
}

#[test]
fn finds_more_body_local_item_symbols_at_offsets() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_more_body_local_symbols"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    type Al$symbol_alias_decl$ias = GlobalId;
    const DE$symbol_const_decl$FAULT: Alias = GlobalId;
    static CU$symbol_static_decl$RRENT: GlobalId = GlobalId;
    fn hel$symbol_fn_decl$per() -> Alias {
        DEFAULT
    }
    enum Action {
        Sta$symbol_variant_decl$rt,
        Stop,
    }

    let _typed: Al$symbol_alias_path$ias = hel$symbol_fn_path$per();
    let _default = DE$symbol_const_path$FAULT;
    let _current = CU$symbol_static_path$RRENT;
    let _action = Action::Sta$symbol_variant_path$rt;
}
"#,
        &[
            AnalysisQuery::symbol("symbol at local alias declaration", "symbol_alias_decl"),
            AnalysisQuery::symbol("symbol at local const declaration", "symbol_const_decl"),
            AnalysisQuery::symbol("symbol at local static declaration", "symbol_static_decl"),
            AnalysisQuery::symbol("symbol at local function declaration", "symbol_fn_decl"),
            AnalysisQuery::symbol("symbol at local variant declaration", "symbol_variant_decl"),
            AnalysisQuery::symbol("symbol at local alias path", "symbol_alias_path"),
            AnalysisQuery::symbol("symbol at local function path", "symbol_fn_path"),
            AnalysisQuery::symbol("symbol at local const path", "symbol_const_path"),
            AnalysisQuery::symbol("symbol at local static path", "symbol_static_path"),
            AnalysisQuery::symbol("symbol at local variant path", "symbol_variant_path"),
        ],
        expect![[r#"
            symbol at local alias declaration
            - type fn analysis_more_body_local_symbols[lib]::crate::use_it::Alias @ 4:10-4:15

            symbol at local const declaration
            - const DEFAULT @ 5:11-5:18

            symbol at local static declaration
            - static CURRENT @ 6:12-6:19

            symbol at local function declaration
            - fn helper @ 7:8-7:14

            symbol at local variant declaration
            - variant Start @ 11:9-11:14

            symbol at local alias path
            - body path Alias @ 15:17-15:22

            symbol at local function path
            - expr path helper @ 15:25-15:31

            symbol at local const path
            - expr path DEFAULT @ 16:20-16:27

            symbol at local static path
            - expr path CURRENT @ 17:20-17:27

            symbol at local variant path
            - body value path Action::Start @ 18:27-18:32
        "#]],
    );
}
