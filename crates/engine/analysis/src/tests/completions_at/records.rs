use expect_test::expect;

use super::super::utils::{AnalysisQuery, check_analysis_queries};
#[test]
fn completes_record_constructor_paths() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_record_constructor_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod api {
    pub struct User {
        pub id: u8,
    }

    pub enum Action {
        Start { id: u8 },
    }

    pub fn build_user() -> User {
        User { id: 0 }
    }
}

pub struct LocalUser {
    id: u8,
}

pub fn use_it() {
    enum LocalAction {
        Start { id: u8 },
    }

    let _local = Local$local_ctor$ { id: 0 };
    let _local_variant = LocalAction::Sta$local_variant_ctor$ { id: 0 };
    let _record = api::Us$record_ctor$ { id: 0 };
    let _variant = api::Action::Sta$variant_ctor$ { id: 0 };
}
"#,
        &[
            AnalysisQuery::complete("unqualified record constructor completions", "local_ctor"),
            AnalysisQuery::complete(
                "body-local record variant constructor completions",
                "local_variant_ctor",
            ),
            AnalysisQuery::complete("qualified record constructor completions", "record_ctor"),
            AnalysisQuery::complete("record variant constructor completions", "variant_ctor"),
        ],
        expect![[r#"
            unqualified record constructor completions
            - struct LocalUser
            - module api
            - fn use_it

            body-local record variant constructor completions
            - variant Start

            qualified record constructor completions
            - enum Action
            - struct User

            record variant constructor completions
            - variant Start
        "#]],
    );
}

#[test]
fn completes_record_fields_for_semantic_and_body_local_enum_variants() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_enum_variant_record_field_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Action {
    Named { code: u8, message: String },
}

pub fn inspect(action: Action) {
    let _ = Action::Named { co$semantic_expr$: 1, message: String::new() };
    let Action::Named { me$semantic_pat$: _, code: _ } = action;

    enum Local {
        Named { alpha: u8, beta: u8 },
    }
    let local = Local::Named { al$local_expr$: 1, beta: 2 };
    let Local::Named { be$local_pat$: _, alpha: _ } = local;
}
"#,
        &[
            AnalysisQuery::complete("semantic variant expression fields", "semantic_expr"),
            AnalysisQuery::complete("semantic variant pattern fields", "semantic_pat"),
            AnalysisQuery::complete("local variant expression fields", "local_expr"),
            AnalysisQuery::complete("local variant pattern fields", "local_pat"),
        ],
        expect![[r#"
            semantic variant expression fields
            - field code

            semantic variant pattern fields
            - field message

            local variant expression fields
            - field alpha

            local variant pattern fields
            - field beta
        "#]],
    );
}

#[test]
fn completes_record_fields_across_literal_and_pattern_contexts() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_record_field_context_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User {
    pub id: u8,
    pub name: u8,
    pub active: bool,
}

pub struct Users;

pub fn use_it(id: u8, user: User, users: Users) {
    let _with_prefix = User { id, na$literal_prefix$ };
    let _empty = User { id, $literal_empty$ };
    let _defaults = User { ..$literal_defaults$ };

    let User { id, na$pattern_prefix$ } = user;
    let User { ..$pattern_rest$ } = user;

    if let User { id, na$if_field$ } = user {}

    while let User { ac$while_field$ } = user {}

    for User { id, na$for_field$ } in users {}
}
"#,
        &[
            AnalysisQuery::complete("record literal prefix completions", "literal_prefix"),
            AnalysisQuery::complete("record literal empty completions", "literal_empty"),
            AnalysisQuery::complete("record literal defaults completions", "literal_defaults"),
            AnalysisQuery::complete("record pattern prefix completions", "pattern_prefix"),
            AnalysisQuery::complete("record pattern rest completions", "pattern_rest"),
            AnalysisQuery::complete("if let record pattern fields", "if_field"),
            AnalysisQuery::complete("while let record pattern fields", "while_field"),
            AnalysisQuery::complete("for record pattern fields", "for_field"),
        ],
        expect![[r#"
            record literal prefix completions
            - field active
            - field name

            record literal empty completions
            - field active
            - field name

            record literal defaults completions
            - <none>

            record pattern prefix completions
            - field active
            - field name

            record pattern rest completions
            - <none>

            if let record pattern fields
            - field active
            - field name

            while let record pattern fields
            - field active
            - field id
            - field name

            for record pattern fields
            - field active
            - field name
        "#]],
    );
}

#[test]
fn completes_body_local_record_literal_fields() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_record_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    struct Local {
        left: u8,
        right: u8,
    }

    let _value = Local { $0 };
}
"#,
        &[AnalysisQuery::complete(
            "body-local record literal completions",
            "0",
        )],
        expect![[r#"
            body-local record literal completions
            - field left
            - field right
        "#]],
    );
}

#[test]
fn keeps_record_field_value_positions_as_value_completions() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_record_value_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User {
    pub id: u8,
    pub name: u8,
}

pub fn use_it(user_value: u8) {
    let _value = User { id: us$0, name: 0 };
}
"#,
        &[AnalysisQuery::complete(
            "record field value completions",
            "0",
        )],
        expect![[r#"
            record field value completions
            - struct User
            - fn use_it
            - variable user_value
        "#]],
    );
}
