use expect_test::expect;

use super::utils::{AnalysisQuery, check_analysis_queries};

#[test]
fn resolves_type_trait_and_trait_method_implementations() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_goto_implementation"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Account;
pub struct UserName;

pub trait Na$impl_trait$med {
    fn na$impl_trait_method$me(&self) -> UserName;
}

impl Us$impl_type$er {
    pub fn n$impl_inherent$ew() -> Self {
        User
    }
}

impl Named for User {
    fn name(&self) -> UserName {
        missing()
    }
}

impl Named for Account {
    fn name(&self) -> UserName {
        missing()
    }
}

pub fn use_it(user: User) {
    let _again: User = User::n$impl_inherent_use$ew();
    let _name = user.na$impl_trait_call$me();
}
"#,
        &[
            AnalysisQuery::goto_impl("goto implementations of type", "impl_type"),
            AnalysisQuery::goto_impl("goto implementations of trait", "impl_trait"),
            AnalysisQuery::goto_impl("goto implementations of trait method", "impl_trait_method"),
            AnalysisQuery::goto_impl("goto inherent implementation", "impl_inherent"),
            AnalysisQuery::goto_impl("goto inherent implementation use", "impl_inherent_use"),
            AnalysisQuery::goto_impl("goto trait implementation from call", "impl_trait_call"),
        ],
        expect![[r#"
            goto implementations of type
            - impl Named for User @ 15:1-19:2
            - impl User @ 9:1-13:2

            goto implementations of trait
            - impl Named for Account @ 21:1-25:2
            - impl Named for User @ 15:1-19:2

            goto implementations of trait method
            - fn name @ 16:8-16:12
            - fn name @ 22:8-22:12

            goto inherent implementation
            - fn new @ 10:12-10:15

            goto inherent implementation use
            - fn new @ 10:12-10:15

            goto trait implementation from call
            - fn name @ 16:8-16:12
        "#]],
    );
}

#[test]
fn resolves_body_local_type_implementations() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_goto_implementation"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    struct Us$impl_local_type$er;

    impl User {
        fn id(&self) {}
    }

    let us$impl_local_binding$er: User;
}
"#,
        &[
            AnalysisQuery::goto_impl(
                "goto body-local implementations from type",
                "impl_local_type",
            ),
            AnalysisQuery::goto_impl(
                "goto body-local implementations from binding",
                "impl_local_binding",
            ),
        ],
        expect![[r#"
            goto body-local implementations from type
            - impl User @ 4:5-6:6

            goto body-local implementations from binding
            - impl User @ 4:5-6:6
        "#]],
    );
}

#[test]
fn filters_trait_method_call_implementations_by_receiver_generic_args() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_goto_implementation_trait_impl_generics"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Account;
pub struct Wrapper<T>(T);

pub trait Named {
    fn name(&self);
}

impl Named for Wrapper<User> {
    fn name(&self) {}
}

impl Named for Wrapper<Account> {
    fn name(&self) {}
}

pub fn use_it(account: Wrapper<Account>) {
    account.na$impl_account_call$me();
}
"#,
        &[AnalysisQuery::goto_impl(
            "goto trait implementation from generic receiver call",
            "impl_account_call",
        )],
        expect![[r#"
            goto trait implementation from generic receiver call
            - fn name @ 14:8-14:12
        "#]],
    );
}
