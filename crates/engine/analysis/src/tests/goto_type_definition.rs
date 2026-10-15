use expect_test::expect;

use super::utils::{check_analysis, check_analysis_with_fake_sysroot};

#[test]
fn resolves_body_expression_type_definitions() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_goto_type_body"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Profile;

pub struct Account {
    profile: Profile,
}

pub fn make_user() -> User {
    User
}

pub fn use_it(account: Account) {
    let local$0: User = make_user(); // goto_type("goto type from binding declaration")
    let _again: User = loc$0al; // goto_type("goto type from local usage")
    let _profile: Profile = account.pro$0file; // goto_type("goto type from field access")
    let _made = make_user($0); // goto_type("goto type from call expression")
}
"#,
        expect![[r#"
            struct User @ 1:12-1:16 // goto type from binding declaration
            struct User @ 1:12-1:16 // goto type from local usage
            struct Profile @ 2:12-2:19 // goto type from field access
            struct User @ 1:12-1:16 // goto type from call expression
        "#]],
    );
}

#[test]
fn resolves_type_definitions_through_references_try_and_await_wrappers() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_goto_type_wrappers"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Error;
pub struct User;

pub fn load_user() -> Result<User, Error> {
    todo!()
}

pub async fn load_user_async() -> User {
    User
}

pub async fn use_it(user: User) -> Result<(), Error> {
    let _borrowed = (&user)$0; // goto_type("goto type from reference wrapper").in_lib("analysis_goto_type_wrappers")
    let borrowed_twice = &&user;
    let _borrowed_twice = borrowed_twice$0; // goto_type("goto type from double reference").in_lib("analysis_goto_type_wrappers")
    let _loaded = load_user()?$0; // goto_type("goto type from try wrapper").in_lib("analysis_goto_type_wrappers")
    let _awaited = load_user_async().await$0; // goto_type("goto type from await wrapper").in_lib("analysis_goto_type_wrappers")
    Result::Ok(())
}
"#,
        expect![[r#"
            struct User @ 2:12-2:16 // goto type from reference wrapper
            struct User @ 2:12-2:16 // goto type from double reference
            struct User @ 2:12-2:16 // goto type from try wrapper
            struct User @ 2:12-2:16 // goto type from await wrapper
        "#]],
    );
}

#[test]
fn resolves_signature_type_definitions() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_goto_type_signature"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Profile;

pub struct Account {
    profile$0: Pro$1file, // goto_type("goto type from field declaration"), goto_type("goto type from field type path")
}

pub fn make(user: Us$0er) -> Us$1er { // goto_type("goto type from parameter"), goto_type("goto type from return")
    user
}

impl User {
    pub fn new() -> Se$0lf { // goto_type("goto type from Self")
        User
    }
}
"#,
        expect![[r#"
            struct Profile @ 2:12-2:19 // goto type from field declaration
            struct Profile @ 2:12-2:19 // goto type from field type path
            struct User @ 1:12-1:16 // goto type from parameter
            struct User @ 1:12-1:16 // goto type from return
            struct User @ 1:12-1:16 // goto type from Self
        "#]],
    );
}

#[test]
fn resolves_nominal_types_inside_a_transparent_generic_alias() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_goto_type_alias"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Wrapper<T>(pub T);
pub struct User;
pub type Alias<T> = Wrapper<T>;

pub fn inspect(_: Ali$0as<User>) { // goto_type("goto nominal types inside a signature alias")
    let _: Ali$0as<User>; // goto_type("goto nominal types inside a body alias")
}
"#,
        expect![[r#"
            // goto nominal types inside a signature alias
            struct User @ 2:12-2:16
            struct Wrapper @ 1:12-1:19

            // goto nominal types inside a body alias
            struct User @ 2:12-2:16
            struct Wrapper @ 1:12-1:19
        "#]],
    );
}

#[test]
fn resolves_body_local_type_definitions() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_goto_type_body_local"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn use_it() {
    struct User;

    let local$0: User = User; // goto_type("goto type from body-local binding declaration")
    let _again: User = loc$0al; // goto_type("goto type from body-local usage")
}
"#,
        expect![[r#"
            struct User @ 4:12-4:16 // goto type from body-local binding declaration
            struct User @ 4:12-4:16 // goto type from body-local usage
        "#]],
    );
}

#[test]
fn resolves_body_local_generic_impl_method_return_type_definitions() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_goto_type_body_local_impl_generic"
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
    let _value = wrapper.get($0); // goto_type("goto type from body-local generic impl method")
}
"#,
        expect![[r#"
            struct User @ 2:12-2:16 // goto type from body-local generic impl method
        "#]],
    );
}

#[test]
fn resolves_enum_pattern_payload_type_definitions() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_goto_type_enum_pattern"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub enum Option<T> {
    Some(T),
    None,
}

pub fn use_it(maybe: Option<User>) {
    let Some(value) = maybe else { return; };
    let _again = val$0ue; // goto_type("goto type from enum pattern payload")
}
"#,
        expect![[r#"
            struct User @ 1:12-1:16 // goto type from enum pattern payload
        "#]],
    );
}
