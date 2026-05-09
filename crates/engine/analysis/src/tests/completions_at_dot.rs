use expect_test::expect;

use super::utils::{AnalysisQuery, check_analysis_queries};

#[test]
fn completes_inherent_and_trait_methods_at_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Named {
    fn trait_name(&self);
    fn associated() {}
}

pub struct User;

impl User {
    pub fn new() -> Self {
        User
    }

    pub fn id(&self) {}

    pub fn touch(&mut self) {}
}

impl Named for User {
    fn trait_name(&self) {}
}

pub fn use_it(user: User) {
    user.$0id();
}
"#,
        &[AnalysisQuery::complete("dot completions", "0")],
        expect![[r#"
            dot completions
            - inherent_method id
            - inherent_method touch
            - trait_method trait_name
        "#]],
    );
}

#[test]
fn completes_methods_at_bare_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_bare_dot_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Named {
    fn trait_name(&self);
}

pub struct User;

impl User {
    pub fn id(&self) {}

    pub fn touch(&mut self) {}
}

impl Named for User {
    fn trait_name(&self) {}
}

pub fn use_it(user: User) {
    user.$0;
}
"#,
        &[AnalysisQuery::complete("bare dot completions", "0")],
        expect![[r#"
            bare dot completions
            - inherent_method id
            - inherent_method touch
            - trait_method trait_name
        "#]],
    );
}

#[test]
fn completes_through_references_try_and_await_wrappers() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_wrapper_completion"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Result<T, E> {
    Ok(T),
    Err(E),
}

pub struct Error;

pub struct User {
    profile: Profile,
}

impl User {
    pub fn id(&self) {}
}

pub struct Profile;

pub fn load_user() -> Result<User, Error> {
    todo!()
}

pub async fn load_user_async() -> User {
    User { profile: Profile }
}

pub async fn use_it(user: User) -> Result<(), Error> {
    (&user).$reference$;
    load_user()?.$try$;
    load_user_async().await.$await$;
    Result::Ok(())
}
"#,
        &[
            AnalysisQuery::complete("reference completions", "reference"),
            AnalysisQuery::complete("try completions", "try"),
            AnalysisQuery::complete("await completions", "await"),
        ],
        expect![[r#"
            reference completions
            - inherent_method id
            - field profile

            try completions
            - inherent_method id
            - field profile

            await completions
            - inherent_method id
            - field profile
        "#]],
    );
}

#[test]
fn completes_methods_for_bin_root_library_type() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_bin_completion"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[[bin]]
name = "analysis-bin-completion"
path = "src/main.rs"

//- /src/lib.rs
pub struct Api;

impl Api {
    pub fn ping(&self) {}
    pub fn work(&self) {}
}

//- /src/main.rs
fn main() {
    let api: analysis_bin_completion::Api = todo!();
    api.$0;
}
"#,
        &[AnalysisQuery::complete("bin root completions", "0").in_bin("analysis_bin_completion")],
        expect![[r#"
            bin root completions
            - inherent_method ping
            - inherent_method work
        "#]],
    );
}

#[test]
fn does_not_trigger_inside_method_arguments() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_dot_range"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

impl User {
    pub fn id(&self, _value: u8) {}

    pub fn touch(&self) {}
}

pub fn use_it(user: User) {
    user.id($inside_arg$0);
}
"#,
        &[AnalysisQuery::complete(
            "completion inside method argument",
            "inside_arg",
        )],
        expect![[r#"
            completion inside method argument
            - <none>
        "#]],
    );
}

#[test]
fn preserves_distinct_same_name_candidates() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_duplicates"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Named {
    fn label(&self);
}

pub trait Displayed {
    fn label(&self);
}

pub struct User;

impl User {
    pub fn label(&self) {}
}

impl Named for User {
    fn label(&self) {}
}

impl Displayed for User {
    fn label(&self) {}
}

pub fn use_it(user: User) {
    user.$0label();
}
"#,
        &[AnalysisQuery::complete("same-name completions", "0")],
        expect![[r#"
            same-name completions
            - inherent_method label
            - trait_method label
            - trait_method label
        "#]],
    );
}

#[test]
fn does_not_complete_concrete_impl_methods_for_wrong_generic_args() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_concrete_impl_completion"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Error;

pub struct Wrapper<T> {
    value: T,
}

impl<T> Wrapper<T> {
    pub fn generic(&self) {}
}

impl Wrapper<User> {
    pub fn user_only(&self) {}
}

pub trait UserOnlyTrait {
    fn trait_user_only(&self);
}

impl UserOnlyTrait for Wrapper<User> {
    fn trait_user_only(&self) {}
}

pub fn use_it(error: Wrapper<Error>) {
    error.$0;
}
"#,
        &[AnalysisQuery::complete(
            "wrong generic arg completions",
            "0",
        )],
        expect![[r#"
            wrong generic arg completions
            - inherent_method generic
            - field value
        "#]],
    );
}

#[test]
fn marks_generic_trait_method_completions_as_maybe() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_generic_trait_completion"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub struct Wrapper<T> {
    value: T,
}

pub trait GenericNamed {
    fn generic_trait_name(&self);
}

impl<T> GenericNamed for Wrapper<T> {
    fn generic_trait_name(&self) {}
}

pub trait BoundNamed {
    fn bounded_trait_name(&self);
}

pub trait Required {}

impl<T> BoundNamed for Wrapper<T>
where
    T: Required,
{
    fn bounded_trait_name(&self) {}
}

pub fn use_it(wrapper: Wrapper<User>) {
    wrapper.$0;
}
"#,
        &[AnalysisQuery::complete("generic trait completions", "0")],
        expect![[r#"
            generic trait completions
            - trait_method bounded_trait_name (maybe)
            - trait_method generic_trait_name (maybe)
            - field value
        "#]],
    );
}

#[test]
fn completes_methods_after_field_receiver() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_field_receiver_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Profile;

impl Profile {
    pub fn display(&self) {}
}

pub struct User {
    pub profile: Profile,
}

pub fn use_it(user: User) {
    user.profile.$0;
}
"#,
        &[AnalysisQuery::complete("field receiver completions", "0")],
        expect![[r#"
            field receiver completions
            - inherent_method display
        "#]],
    );
}

#[test]
fn completes_fields_at_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_field_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Profile;

pub struct User {
    pub profile: Profile,
}

pub fn use_it(user: User) {
    user.$0;
}
"#,
        &[AnalysisQuery::complete("field completions", "0")],
        expect![[r#"
            field completions
            - field profile
        "#]],
    );
}

#[test]
fn completes_body_local_struct_fields_at_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_field_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn use_it() {
    struct User {
        id: UserId,
        profile: Profile,
    }
    struct Pair(UserId, Profile);
    struct UserId;
    struct Profile;

    let user: User;
    user.$0;

    let pair: Pair;
    pair.$tuple$;
}
"#,
        &[
            AnalysisQuery::complete("body-local field completions", "0"),
            AnalysisQuery::complete("body-local tuple field completions", "tuple"),
        ],
        expect![[r#"
            body-local field completions
            - field id
            - field profile

            body-local tuple field completions
            - field 0
            - field 1
        "#]],
    );
}

#[test]
fn completes_body_local_impl_methods_at_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_impl_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User {
        id: GlobalId,
    }

    impl User {
        fn id(&self) -> GlobalId {
            missing()
        }

        fn associated() -> GlobalId {
            missing()
        }
    }

    let user: User;
    user.$0;
}
"#,
        &[AnalysisQuery::complete("body-local impl completions", "0")],
        expect![[r#"
            body-local impl completions
            - field id
            - inherent_method id
        "#]],
    );
}

#[test]
fn completes_body_local_impl_methods_from_nested_blocks() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_nested_body_local_impl_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User {
        id: GlobalId,
    }

    {
        impl User {
            fn id(&self) -> GlobalId {
                missing()
            }
        }
    }

    let user: User;
    user.$0;
}
"#,
        &[AnalysisQuery::complete(
            "nested body-local impl completions",
            "0",
        )],
        expect![[r#"
            nested body-local impl completions
            - field id
            - inherent_method id
        "#]],
    );
}

#[test]
fn completes_body_local_generic_impl_method_return_and_field_receivers() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_generic_impl_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    struct Id;
    struct Error;
    struct User {
        id: Id,
    }

    impl User {
        fn label(&self) {}
    }

    struct Wrapper<T> {
        value: T,
    }

    impl<U> Wrapper<U> {
        fn get(&self) -> U {
            missing()
        }
    }

    impl Wrapper<User> {
        fn user_only(&self) -> User {
            missing()
        }
    }

    let wrapper: Wrapper<User>;
    wrapper.get().$method_return$;
    wrapper.value.$field_receiver$;

    let error: Wrapper<Error>;
    error.$wrong_receiver$;
}
"#,
        &[
            AnalysisQuery::complete("generic method return completions", "method_return"),
            AnalysisQuery::complete("generic field receiver completions", "field_receiver"),
            AnalysisQuery::complete("wrong generic receiver completions", "wrong_receiver"),
        ],
        expect![[r#"
            generic method return completions
            - field id
            - inherent_method label

            generic field receiver completions
            - field id
            - inherent_method label

            wrong generic receiver completions
            - inherent_method get
            - field value
        "#]],
    );
}

#[test]
fn completes_fields_and_methods_after_enum_pattern_payloads() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_enum_pattern_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Id;

pub struct User {
    id: Id,
}

impl User {
    fn label(&self) {}
}

pub enum Option<T> {
    Some(T),
    None,
}

pub fn use_it(maybe: Option<User>) {
    let Some(value) = maybe else { return; };
    value.$let_payload$;

    match maybe {
        Some(user) => user.$match_payload$,
        None => {}
    }
}
"#,
        &[
            AnalysisQuery::complete("let pattern payload completions", "let_payload"),
            AnalysisQuery::complete("match pattern payload completions", "match_payload"),
        ],
        expect![[r#"
            let pattern payload completions
            - field id
            - inherent_method label

            match pattern payload completions
            - field id
            - inherent_method label
        "#]],
    );
}

#[test]
fn completes_tuple_fields_at_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_tuple_field_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Left;
pub struct Right;

pub struct Pair(pub Left, pub Right);

pub fn use_it(pair: Pair) {
    pair.$0;
}
"#,
        &[AnalysisQuery::complete("tuple field completions", "0")],
        expect![[r#"
            tuple field completions
            - field 0
            - field 1
        "#]],
    );
}
