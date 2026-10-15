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
            local local @ 8:9-8:14 // symbol at declaration
            expr path local @ 9:24-9:29 // symbol at local path
            expr path helper @ 10:23-10:29 // symbol at item path
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
            struct User @ 1:12-1:16 // symbol at struct declaration
            trait Named @ 3:11-3:16 // symbol at trait declaration
            type path User @ 7:19-7:23 // symbol at parameter type
            type path User @ 7:28-7:32 // symbol at return type
        "#]],
    );
}

#[test]
fn finds_body_macro_call_symbols_at_offsets() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_macro_symbol_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Text;

pub fn helper(value: u64) -> Text { Text }

macro_rules! make_text {
    ($value:expr) => { helper($value) };
}

pub fn use_it(input: u64) {
    let _text = make_$symbol_macro$text!(input);
}
"#,
        &[AnalysisQuery::symbol(
            "symbol at body macro call",
            "symbol_macro",
        )],
        expect![[r#"
            macro make_text @ 10:17-10:26 // symbol at body macro call
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
            struct User @ 4:12-4:16 // symbol at local struct declaration
            type path User @ 5:17-5:21 // symbol at local type path
            expr path User @ 5:24-5:28 // symbol at local constructor
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
            type_alias Alias @ 4:10-4:15 // symbol at local alias declaration
            const DEFAULT @ 5:11-5:18 // symbol at local const declaration
            static CURRENT @ 6:12-6:19 // symbol at local static declaration
            fn helper @ 7:8-7:14 // symbol at local function declaration
            variant Start @ 11:9-11:14 // symbol at local variant declaration
            type path Alias @ 15:17-15:22 // symbol at local alias path
            expr path helper @ 15:25-15:31 // symbol at local function path
            expr path DEFAULT @ 16:20-16:27 // symbol at local const path
            expr path CURRENT @ 17:20-17:27 // symbol at local static path
            expr path Action::Start @ 18:19-18:32 // symbol at local variant path
        "#]],
    );
}
