use expect_test::expect;

use crate::tests::utils::check_analysis;
#[test]
fn completes_record_constructor_paths() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_record_constructor_path_completions
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

    let _local = Local$0 { id: 0 }; // complete("unqualified record constructor completions")
    let _local_variant = LocalAction::Sta$0 { id: 0 }; // complete("body-local record variant constructor completions")
    let _record = api::Us$0 { id: 0 }; // complete("qualified record constructor completions")
    let _variant = api::Action::Sta$0 { id: 0 }; // complete("record variant constructor completions")
}
"#,
        expect![[r#"
            // unqualified record constructor completions
            st LocalUser
            md api
            fn use_it

            ev Start // body-local record variant constructor completions

            // qualified record constructor completions
            en Action
            st User

            ev Start // record variant constructor completions
        "#]],
    );
}

#[test]
fn completes_record_fields_for_semantic_and_body_local_enum_variants() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_enum_variant_record_field_completions
pub enum Action {
    Named { code: u8, message: String },
}

pub fn inspect(action: Action) {
    let _ = Action::Named { co$0: 1, message: String::new() }; // complete("semantic variant expression fields")
    let Action::Named { me$0: _, code: _ } = action; // complete("semantic variant pattern fields")

    enum Local {
        Named { alpha: u8, beta: u8 },
    }
    let local = Local::Named { al$0: 1, beta: 2 }; // complete("local variant expression fields")
    let Local::Named { be$0: _, alpha: _ } = local; // complete("local variant pattern fields")
}
"#,
        expect![[r#"
            fd code // semantic variant expression fields
            fd message // semantic variant pattern fields
            fd alpha // local variant expression fields
            fd beta // local variant pattern fields
        "#]],
    );
}

#[test]
fn completes_record_fields_across_literal_and_pattern_contexts() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_record_field_context_completions
pub struct User {
    pub id: u8,
    pub name: u8,
    pub active: bool,
}

pub struct Users;

pub fn use_it(id: u8, user: User, users: Users) {
    let _with_prefix = User { id, na$0 }; // complete("record literal prefix completions")
    let _empty = User { id, $0 }; // complete("record literal empty completions")
    let _defaults = User { ..$0 }; // complete("record literal defaults completions")

    let User { id, na$0 } = user; // complete("record pattern prefix completions")
    let User { ..$0 } = user; // complete("record pattern rest completions")

    if let User { id, na$0 } = user {} // complete("if let record pattern fields")

    while let User { ac$0 } = user {} // complete("while let record pattern fields")

    for User { id, na$0 } in users {} // complete("for record pattern fields")
}
"#,
        expect![[r#"
            // record literal prefix completions
            fd active
            fd name

            // record literal empty completions
            fd active
            fd name

            <none> // record literal defaults completions

            // record pattern prefix completions
            fd active
            fd name

            <none> // record pattern rest completions

            // if let record pattern fields
            fd active
            fd name

            // while let record pattern fields
            fd active
            fd id
            fd name

            // for record pattern fields
            fd active
            fd name
        "#]],
    );
}

#[test]
fn completes_body_local_record_literal_fields() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_local_record_completions
pub fn use_it() {
    struct Local {
        left: u8,
        right: u8,
    }

    let _value = Local { $0 }; // complete("body-local record literal completions")
}
"#,
        expect![[r#"
            // body-local record literal completions
            fd left
            fd right
        "#]],
    );
}

#[test]
fn keeps_record_field_value_positions_as_value_completions() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_record_value_completions
pub struct User {
    pub id: u8,
    pub name: u8,
}

pub fn use_it(user_value: u8) {
    let _value = User { id: us$0, name: 0 }; // complete("record field value completions")
}
"#,
        expect![[r#"
            // record field value completions
            st User
            fn use_it
            vr user_value
        "#]],
    );
}
