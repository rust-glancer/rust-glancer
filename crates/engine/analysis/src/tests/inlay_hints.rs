use expect_test::expect;

use super::utils::{InlayHintsQuery, check_inlay_hints, check_inlay_hints_with_fake_sysroot};

#[test]
fn shows_inferred_local_binding_types() {
    check_inlay_hints(
        r#"
//- /Cargo.toml
[package]
name = "analysis_type_hints"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Option<T> {
    pub value: T,
}

pub fn helper() -> User {
    User
}

pub fn use_it() {
    let user = helper();
    let explicit: User = helper();
    let wrapped: Option<User> = missing();
    let value = wrapped.value;
    let unknown = missing();
}
"#,
        InlayHintsQuery::new("type hints", "/src/lib.rs"),
        expect![[r#"
            type hints
            - `: User` @ 11:9-11:13
            - `: User` @ 14:9-14:14
        "#]],
    );
}

#[test]
fn shows_type_hints_inside_bin_roots() {
    check_inlay_hints(
        r#"
//- /Cargo.toml
[package]
name = "analysis_bin_type_hints"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "app"
path = "src/main.rs"

//- /src/main.rs
struct User;

fn make_user() -> User {
    User
}

fn main() {
    let user = make_user();
}
"#,
        InlayHintsQuery::new("bin type hints", "/src/main.rs").in_bin("analysis_bin_type_hints"),
        expect![[r#"
            bin type hints
            - `: User` @ 8:9-8:13
        "#]],
    );
}

#[test]
fn skips_inlay_hints_for_generated_body_macro_internals() {
    check_inlay_hints(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_macro_inlay_hints"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Text;

pub fn helper(value: u64) -> Text {
    Text
}

macro_rules! make_text {
    ($value:expr) => { helper($value) };
}

pub fn demo(input: u64) {
    let text = make_text!(input);
}
"#,
        InlayHintsQuery::new("body macro inlay hints", "/src/lib.rs"),
        expect![[r#"
            body macro inlay hints
            - `: Text` @ 12:9-12:13
        "#]],
    );
}

#[test]
fn shows_type_hints_for_pattern_bindings_with_known_types() {
    check_inlay_hints(
        r#"
//- /Cargo.toml
[package]
name = "analysis_pattern_type_hints"
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
    value;

    let Message::User { profile } = message else { return; };
    profile;
}

pub fn match_it(maybe: Option<User>) {
    match maybe {
        Option::Some(user) => {
            user;
        }
        Option::None => {}
    }
}
"#,
        InlayHintsQuery::new("pattern type hints", "/src/lib.rs"),
        expect![[r#"
            pattern type hints
            - `: User` @ 15:14-15:19
            - `: Profile` @ 18:25-18:32
            - `: User` @ 24:22-24:26
        "#]],
    );
}

#[test]
fn shows_type_hints_for_for_loop_bindings_with_known_items() {
    check_inlay_hints_with_fake_sysroot(
        r#"
//- /Cargo.toml
[workspace]
members = ["app"]
resolver = "3"

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /app/src/lib.rs
pub struct Package;
pub struct UserId;

pub fn use_it(packages: &[Package], array: [Package; 3], pairs: [(Package, UserId); 2]) {
    for borrowed in packages {
        borrowed;
    }

    for owned in array {
        owned;
    }

    for (package, user_id) in pairs {
        package;
        user_id;
    }
}
"#,
        InlayHintsQuery::new("for loop type hints", "/app/src/lib.rs").in_lib("app"),
        expect![[r#"
            for loop type hints
            - `: &Package` @ 5:9-5:17
            - `: Package` @ 9:9-9:14
            - `: Package` @ 13:10-13:17
            - `: UserId` @ 13:19-13:26
        "#]],
    );
}

#[test]
fn applies_expression_type_hint_policy_to_method_chains() {
    check_inlay_hints(
        r#"
//- /Cargo.toml
[package]
name = "analysis_method_chain_expression_type_hints"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Factory;
pub struct User;
pub struct Profile;
pub struct UserName;

impl Factory {
    pub fn build(&self) -> User {
        User
    }
}

impl User {
    pub fn profile(self) -> Profile {
        Profile
    }
}

impl Profile {
    pub fn name(self) -> UserName {
        UserName
    }
}

pub fn multiline(factory: Factory) {
    let name: UserName = factory
        .build()
        .profile()
        .name();
}

pub fn inline_and_unknown(factory: Factory) {
    let inline: UserName = factory.build().profile().name();
    let unknown: UserName = missing()
        .build()
        .profile()
        .name();
}
"#,
        InlayHintsQuery::new("method chain expression type hint policy", "/src/lib.rs"),
        expect![[r#"
            method chain expression type hint policy
            - `User` @ 25:26-26:17
            - `Profile` @ 25:26-27:19
        "#]],
    );
}

#[test]
fn applies_closing_brace_hint_policy_to_long_and_short_constructs() {
    check_inlay_hints(
        r#"
//- /Cargo.toml
[package]
name = "analysis_closing_brace_hint_policy"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod outer {
    pub enum Mode {
        Fast,
        Slow,
    }

    pub struct Items;
    pub struct User;

    impl User {
        pub fn process(&self, mode: Mode, items: Items, active: bool) {
            match mode {
                Mode::Fast => {
                    for item in items {
                        while active {
                            loop {
                                ();
                                ();
                                ();
                                ();
                                ();
                                ();
                                ();
                                ();
                                ();
                                ();
                                ();
                                ();
                                ();
                                ();
                                ();
                                ();
                                ();
                                ();
                                ();
                                break;
                            }
                            break;
                        }
                    }
                }
                Mode::Slow => {}
            }
        }
    }
}

// Short constructs deliberately stay below the closing-hint threshold.
pub fn short() {
    ();
}
"#,
        InlayHintsQuery::new("closing brace hint policy", "/src/lib.rs"),
        expect![[r#"
            closing brace hint policy
            - `// loop` @ 37:29-37:30
            - `// while active` @ 39:25-39:26
            - `// for item in items` @ 40:21-40:22
            - `// match mode` @ 43:13-43:14
            - `// fn process` @ 44:9-44:10
            - `// impl User` @ 45:5-45:6
            - `// mod outer` @ 46:1-46:2
        "#]],
    );
}

#[test]
fn applies_parameter_hint_policy_to_resolved_noisy_and_unresolved_calls() {
    check_inlay_hints(
        r#"
//- /Cargo.toml
[package]
name = "analysis_parameter_hint_policy"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn build(scope: u32, annotation: User, initializer: User) -> User {
    initializer
}

pub fn destructured((left, right): (User, User), _: User, normal: User) {}

impl User {
    pub fn update(&self, active: bool, pending_tys: User) {}

    pub fn make(value: User, count: u32) -> User {
        value
    }
}

pub fn use_it(scope: u32, user: User, other: User, normal: User) {
    build(scope, user, other);
    user.update(true, other);
    User::make(user, 10);

    // Destructured, omitted, matching-name, and unresolved parameters are intentionally quiet.
    destructured((user, other), user, normal);
    missing(user);
}
"#,
        InlayHintsQuery::new("parameter hint policy", "/src/lib.rs"),
        expect![[r#"
            parameter hint policy
            - `annotation:` @ 18:18-18:22
            - `initializer:` @ 18:24-18:29
            - `active:` @ 19:17-19:21
            - `pending_tys:` @ 19:23-19:28
            - `value:` @ 20:16-20:20
            - `count:` @ 20:22-20:24
        "#]],
    );
}
