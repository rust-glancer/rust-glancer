use expect_test::expect;

use super::utils::{AnalysisQuery, check_analysis_queries};

#[test]
fn returns_body_expression_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_type_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn helper() -> User {
    User
}

pub fn use_it() {
    let local: User = helper();
    let _typed: User = loc$type_at$al;
}
"#,
        &[AnalysisQuery::ty("type at local", "type_at")],
        expect![[r#"
            type at local
            - nominal struct analysis_type_at[lib]::crate::User
        "#]],
    );
}

#[test]
fn returns_types_for_references_try_and_await_wrappers() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_wrapper_type_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Result<T, E> {
    Ok(T),
    Err(E),
}

pub struct Error;
pub struct User;

pub fn load_user() -> Result<User, Error> {
    todo!()
}

pub async fn load_user_async() -> User {
    User
}

pub async fn use_it(user: User) -> Result<(), Error> {
    let _borrowed = (&user)$type_ref$;
    let _loaded = load_user()?$type_try$;
    let _awaited = load_user_async().await$type_await$;
    Result::Ok(())
}
"#,
        &[
            AnalysisQuery::ty("type at reference wrapper", "type_ref"),
            AnalysisQuery::ty("type at try wrapper", "type_try"),
            AnalysisQuery::ty("type at await wrapper", "type_await"),
        ],
        expect![[r#"
            type at reference wrapper
            - &nominal struct analysis_wrapper_type_at[lib]::crate::User

            type at try wrapper
            - nominal struct analysis_wrapper_type_at[lib]::crate::User

            type at await wrapper
            - nominal struct analysis_wrapper_type_at[lib]::crate::User
        "#]],
    );
}

#[test]
fn returns_binding_declaration_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_binding_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn helper() -> User {
    User
}

pub fn use_it() {
    let typed$type_decl$: User = helper();
}
"#,
        &[AnalysisQuery::ty(
            "type at declaration binding",
            "type_decl",
        )],
        expect![[r#"
            type at declaration binding
            - nominal struct analysis_binding_type[lib]::crate::User
        "#]],
    );
}

#[test]
fn returns_associated_function_and_enum_variant_call_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_associated_path_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Widget;

impl Widget {
    pub fn create() -> Self {
        Widget
    }
}

pub enum Action {
    Configure(Widget),
}

pub fn use_it() {
    let widget = Widget::create($type_assoc_call$);
    let action = Action::Configure(widget)$type_variant_call$;
}
"#,
        &[
            AnalysisQuery::ty("type at associated function call", "type_assoc_call"),
            AnalysisQuery::ty("type at enum variant call", "type_variant_call"),
        ],
        expect![[r#"
            type at associated function call
            - Self struct analysis_associated_path_type[lib]::crate::Widget

            type at enum variant call
            - nominal enum analysis_associated_path_type[lib]::crate::Action
        "#]],
    );
}

#[test]
fn returns_bin_root_dependency_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub struct Thing;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

[lib]
path = "src/lib.rs"

[[bin]]
name = "app-bin"
path = "src/main.rs"

//- /crates/app/src/lib.rs
pub struct Api;

//- /crates/app/src/main.rs
fn main() {
    let thing$type_bin_dep$: dep::Thing = todo!();
}
"#,
        &[AnalysisQuery::ty("type at bin dependency binding", "type_bin_dep").in_bin("app")],
        expect![[r#"
            type at bin dependency binding
            - nominal struct dep[lib]::crate::Thing
        "#]],
    );
}

#[test]
fn returns_field_access_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_field_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Profile;

pub struct User {
    pub profile: Profile,
}

pub fn use_it(user: User) {
    let _typed: Profile = user.pro$type_field$file;
}
"#,
        &[AnalysisQuery::ty("type at field", "type_field")],
        expect![[r#"
            type at field
            - nominal struct analysis_field_type[lib]::crate::Profile
        "#]],
    );
}

#[test]
fn propagates_basic_generic_arguments_through_fields_and_methods() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_generic_type_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Error;

pub struct Option<T> {
    pub value: T,
}

pub struct Vec<T> {
    pub first: T,
}

pub struct Result<T, E> {
    pub ok: T,
    pub err: E,
}

pub struct Wrapper<T> {
    pub value: T,
}

impl<U> Wrapper<U> {
    pub fn get(&self) -> U {
        missing()
    }
}

pub fn use_it() {
    let wrapped: Wrapper<Result<Vec<Option<User>>, Error>>;
    let _result = wrapped.va$type_result$lue;
    let _vec = wrapped.value.o$type_vec$k;
    let _option = wrapped.value.ok.f$type_option$irst;
    let _user = wrapped.value.ok.first.va$type_user$lue;
    let _method = wrapped.ge$type_method$t();
}
"#,
        &[
            AnalysisQuery::ty("generic result field", "type_result"),
            AnalysisQuery::ty("generic vec field", "type_vec"),
            AnalysisQuery::ty("generic option field", "type_option"),
            AnalysisQuery::ty("generic user field", "type_user"),
            AnalysisQuery::ty("generic method return", "type_method"),
        ],
        expect![[r#"
            generic result field
            - nominal struct analysis_generic_type_at[lib]::crate::Result<nominal struct analysis_generic_type_at[lib]::crate::Vec<nominal struct analysis_generic_type_at[lib]::crate::Option<nominal struct analysis_generic_type_at[lib]::crate::User>>, nominal struct analysis_generic_type_at[lib]::crate::Error>

            generic vec field
            - nominal struct analysis_generic_type_at[lib]::crate::Vec<nominal struct analysis_generic_type_at[lib]::crate::Option<nominal struct analysis_generic_type_at[lib]::crate::User>>

            generic option field
            - nominal struct analysis_generic_type_at[lib]::crate::Option<nominal struct analysis_generic_type_at[lib]::crate::User>

            generic user field
            - nominal struct analysis_generic_type_at[lib]::crate::User

            generic method return
            - nominal struct analysis_generic_type_at[lib]::crate::Result<nominal struct analysis_generic_type_at[lib]::crate::Vec<nominal struct analysis_generic_type_at[lib]::crate::Option<nominal struct analysis_generic_type_at[lib]::crate::User>>, nominal struct analysis_generic_type_at[lib]::crate::Error>
        "#]],
    );
}

#[test]
fn does_not_treat_concrete_impl_self_args_as_type_params() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_concrete_impl_args"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Error;

pub struct Wrapper<T> {
    value: T,
}

impl<T> Wrapper<T> {
    pub fn generic(&self) -> T {
        missing()
    }
}

impl Wrapper<User> {
    pub fn user_only(&self) -> User {
        missing()
    }
}

pub fn use_it(user: Wrapper<User>, error: Wrapper<Error>) {
    let _user = user.user$type_user_method$_only();
    let _error = error.gen$type_generic_method$eric();
    let _missing = error.user$type_wrong_method$_only();
}
"#,
        &[
            AnalysisQuery::ty(
                "concrete impl method on matching receiver",
                "type_user_method",
            ),
            AnalysisQuery::ty(
                "generic impl method on concrete receiver",
                "type_generic_method",
            ),
            AnalysisQuery::ty(
                "concrete impl method on wrong receiver",
                "type_wrong_method",
            ),
        ],
        expect![[r#"
            concrete impl method on matching receiver
            - nominal struct analysis_concrete_impl_args[lib]::crate::User

            generic impl method on concrete receiver
            - nominal struct analysis_concrete_impl_args[lib]::crate::Error

            concrete impl method on wrong receiver
            - <unknown>
        "#]],
    );
}

#[test]
fn uses_naive_trait_applicability_for_method_return_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_trait_applicability_type_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
fn missing<T>() -> T {
    loop {}
}

pub struct User;
pub struct Error;

pub struct Wrapper<T> {
    value: T,
}

pub trait BuildUser {
    fn build_user(&self) -> User;
}

impl<T> BuildUser for Wrapper<T> {
    fn build_user(&self) -> User {
        missing()
    }
}

pub trait UserOnlyBuild {
    fn user_only(&self) -> User;
}

impl UserOnlyBuild for Wrapper<User> {
    fn user_only(&self) -> User {
        missing()
    }
}

pub fn use_it(generic: Wrapper<Error>, concrete: Wrapper<Error>) {
    let maybe_user = generic.build_user();
    let _from_maybe = maybe_$type_maybe$user;

    let wrong = concrete.user_only();
    let _from_wrong = wro$type_wrong$ng;
}
"#,
        &[
            AnalysisQuery::ty("maybe trait method return", "type_maybe"),
            AnalysisQuery::ty("concrete trait impl mismatch", "type_wrong"),
        ],
        expect![[r#"
            maybe trait method return
            - nominal struct analysis_trait_applicability_type_at[lib]::crate::User

            concrete trait impl mismatch
            - <unknown>
        "#]],
    );
}

#[test]
fn returns_direct_trait_method_call_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_direct_trait_type_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
fn missing<T>() -> T {
    loop {}
}

pub struct User;
pub struct UserId;

pub trait Identify {
    fn id(&self) -> UserId;
}

impl Identify for User {
    fn id(&self) -> UserId {
        missing()
    }
}

pub fn use_it(user: User) {
    let id = user.id();
    let _again = i$type_direct_trait$d;
}
"#,
        &[AnalysisQuery::ty(
            "direct trait method return",
            "type_direct_trait",
        )],
        expect![[r#"
            direct trait method return
            - nominal struct analysis_direct_trait_type_at[lib]::crate::UserId
        "#]],
    );
}

#[test]
fn returns_body_local_field_access_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_field_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User {
        local_id: GlobalId,
    }

    let user: User;
    let _id: GlobalId = user.loc$type_field$al_id;
}
"#,
        &[AnalysisQuery::ty("type at body-local field", "type_field")],
        expect![[r#"
            type at body-local field
            - nominal struct analysis_body_local_field_type[lib]::crate::GlobalId
        "#]],
    );
}

#[test]
fn propagates_basic_generic_arguments_for_body_local_fields() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_generic_field_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn use_it() {
    struct Slot<T> {
        value: T,
    }

    let slot: Slot<User>;
    let _user = slot.va$type_field$lue;
}
"#,
        &[AnalysisQuery::ty("body-local generic field", "type_field")],
        expect![[r#"
            body-local generic field
            - nominal struct analysis_body_local_generic_field_type[lib]::crate::User
        "#]],
    );
}

#[test]
fn returns_body_local_method_call_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_method_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User;

    impl User {
        fn id(&self) -> GlobalId {
            missing()
        }

        fn again(&self) -> Self {
            missing()
        }
    }

    let user: User;
    let _id: GlobalId = user.i$type_id$d();
    let _again: User = user.a$type_again$gain();
}
"#,
        &[
            AnalysisQuery::ty("type at body-local method", "type_id"),
            AnalysisQuery::ty("type at body-local Self method", "type_again"),
        ],
        expect![[r#"
            type at body-local method
            - nominal struct analysis_body_local_method_type[lib]::crate::GlobalId

            type at body-local Self method
            - local nominal struct fn analysis_body_local_method_type[lib]::crate::use_it::User
        "#]],
    );
}

#[test]
fn returns_nested_body_local_impl_method_call_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_nested_body_local_method_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User;

    {
        impl User {
            fn id(&self) -> GlobalId {
                missing()
            }
        }
    }

    let user: User;
    let _id: GlobalId = user.i$type_id$d();
}
"#,
        &[AnalysisQuery::ty(
            "type at nested body-local method",
            "type_id",
        )],
        expect![[r#"
            type at nested body-local method
            - nominal struct analysis_nested_body_local_method_type[lib]::crate::GlobalId
        "#]],
    );
}

#[test]
fn substitutes_body_local_impl_generics_in_method_returns() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_impl_generic_method_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    struct User;
    struct Wrapper<T> {
        value: T,
    }

    impl<U> Wrapper<U> {
        fn get(&self) -> U {
            missing()
        }
    }

    let wrapper: Wrapper<User>;
    let _value: User = wrapper.ge$type_get$t();
}
"#,
        &[AnalysisQuery::ty(
            "type at body-local generic impl method",
            "type_get",
        )],
        expect![[r#"
            type at body-local generic impl method
            - local nominal struct fn analysis_body_local_impl_generic_method_type[lib]::crate::use_it::User
        "#]],
    );
}

#[test]
fn propagates_enum_variant_payload_types_into_patterns() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_enum_pattern_type_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Profile;

pub enum Option<T> {
    Some(T),
    None,
}

pub enum Message<T> {
    User { profile: T },
    Empty,
}

pub fn use_it(maybe: Option<User>, message: Message<Profile>) {
    let Some(value) = maybe else { return; };
    let _from_let = val$type_let$ue;

    let Message::User { profile } = message else { return; };
    let _from_record = pro$type_record$file;
}

pub fn match_it(maybe: Option<User>) {
    match maybe {
        Option::Some(user) => {
            let _from_match = us$type_match$er;
        }
        Option::None => {}
    }
}
"#,
        &[
            AnalysisQuery::ty("type from tuple variant let pattern", "type_let"),
            AnalysisQuery::ty("type from record variant let pattern", "type_record"),
            AnalysisQuery::ty("type from tuple variant match pattern", "type_match"),
        ],
        expect![[r#"
            type from tuple variant let pattern
            - nominal struct analysis_enum_pattern_type_at[lib]::crate::User

            type from record variant let pattern
            - nominal struct analysis_enum_pattern_type_at[lib]::crate::Profile

            type from tuple variant match pattern
            - nominal struct analysis_enum_pattern_type_at[lib]::crate::User
        "#]],
    );
}

#[test]
fn returns_tuple_field_access_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_tuple_field_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Left;
pub struct Right;

pub struct Pair(pub Left, pub Right);

pub fn use_it(pair: Pair) {
    let _right: Right = pair.$type_tuple_field$1;
}
"#,
        &[AnalysisQuery::ty("type at tuple field", "type_tuple_field")],
        expect![[r#"
            type at tuple field
            - nominal struct analysis_tuple_field_type[lib]::crate::Right
        "#]],
    );
}

#[test]
fn returns_signature_path_and_field_declaration_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_signature_type_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Profile;

pub struct User {
    pub pro$type_field_decl$file: Pro$type_field_path$file,
}

pub fn make(profile: Pro$type_param$file) -> Pro$type_ret$file {
    profile
}
"#,
        &[
            AnalysisQuery::ty("type at field declaration", "type_field_decl"),
            AnalysisQuery::ty("type at field type path", "type_field_path"),
            AnalysisQuery::ty("type at parameter type", "type_param"),
            AnalysisQuery::ty("type at return type", "type_ret"),
        ],
        expect![[r#"
            type at field declaration
            - nominal struct analysis_signature_type_at[lib]::crate::Profile

            type at field type path
            - nominal struct analysis_signature_type_at[lib]::crate::Profile

            type at parameter type
            - nominal struct analysis_signature_type_at[lib]::crate::Profile

            type at return type
            - nominal struct analysis_signature_type_at[lib]::crate::Profile
        "#]],
    );
}

#[test]
fn returns_self_type_in_impl_signatures() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_impl_self_signature_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

impl User {
    pub fn new() -> Se$type_impl_self_signature$lf {
        User
    }
}
"#,
        &[AnalysisQuery::ty(
            "type at impl signature Self",
            "type_impl_self_signature",
        )],
        expect![[r#"
            type at impl signature Self
            - Self struct analysis_impl_self_signature_type[lib]::crate::User
        "#]],
    );
}

#[test]
fn returns_body_local_struct_types_before_module_structs() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_local_struct_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn make() {
    struct User;
    let local$type_binding$: Us$type_local_path$er = User;
    let _again: User = loc$type_local_expr$al;
}

pub fn outside() {
    let outside$type_module_binding$: User = User;
}
"#,
        &[
            AnalysisQuery::ty("type at local binding", "type_binding"),
            AnalysisQuery::ty("type at local type path", "type_local_path"),
            AnalysisQuery::ty("type at local expr", "type_local_expr"),
            AnalysisQuery::ty("type at module binding", "type_module_binding"),
        ],
        expect![[r#"
            type at local binding
            - local nominal struct fn analysis_local_struct_type[lib]::crate::make::User

            type at local type path
            - local nominal struct fn analysis_local_struct_type[lib]::crate::make::User

            type at local expr
            - local nominal struct fn analysis_local_struct_type[lib]::crate::make::User

            type at module binding
            - nominal struct analysis_local_struct_type[lib]::crate::User
        "#]],
    );
}

#[test]
fn returns_body_let_annotation_types_with_body_context() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_annotation_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

impl User {
    pub fn capture(&self) {
        let _this: Se$type_body_self$lf = self;
    }
}

pub fn make() {
    struct User;
    let _: Us$type_wildcard_type$er = User;
    let (_left, _right): (Us$type_tuple_left$er, Us$type_tuple_right$er) = User;
}
"#,
        &[
            AnalysisQuery::ty("type at body Self annotation", "type_body_self"),
            AnalysisQuery::ty("type at wildcard annotation", "type_wildcard_type"),
            AnalysisQuery::ty("type at tuple annotation left", "type_tuple_left"),
            AnalysisQuery::ty("type at tuple annotation right", "type_tuple_right"),
        ],
        expect![[r#"
            type at body Self annotation
            - Self struct analysis_body_annotation_type[lib]::crate::User

            type at wildcard annotation
            - local nominal struct fn analysis_body_annotation_type[lib]::crate::make::User

            type at tuple annotation left
            - local nominal struct fn analysis_body_annotation_type[lib]::crate::make::User

            type at tuple annotation right
            - local nominal struct fn analysis_body_annotation_type[lib]::crate::make::User
        "#]],
    );
}
