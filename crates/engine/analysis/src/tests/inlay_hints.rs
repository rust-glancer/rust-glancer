use expect_test::expect;

use super::utils::{InlayHintsQuery, check_inlay_hints};

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
    check_inlay_hints(
        r#"
//- /Cargo.toml
[workspace]
members = ["core", "app"]
resolver = "3"

//- /core/Cargo.toml
[package]
name = "fake_core"
version = "0.1.0"
edition = "2024"

//- /core/src/lib.rs
pub mod iter {
    pub trait IntoIterator {
        type Item;
    }
}

impl<'a, T> iter::IntoIterator for &'a [T] {
    type Item = &'a T;
}

impl<T, const N: usize> iter::IntoIterator for [T; N] {
    type Item = T;
}

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
core = { package = "fake_core", path = "../core" }

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
fn shows_type_hints_for_multiline_method_chain_segments() {
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

pub fn use_it(factory: Factory) {
    let name: UserName = factory
        .build()
        .profile()
        .name();
}
"#,
        InlayHintsQuery::new("method chain expression type hints", "/src/lib.rs"),
        expect![[r#"
            method chain expression type hints
            - `User` @ 25:26-26:17
            - `Profile` @ 25:26-27:19
        "#]],
    );
}

#[test]
fn skips_expression_type_hints_for_inline_or_unknown_method_chains() {
    check_inlay_hints(
        r#"
//- /Cargo.toml
[package]
name = "analysis_method_chain_expression_type_hint_skips"
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

pub fn use_it(factory: Factory) {
    let inline: UserName = factory.build().profile().name();
    let unknown: UserName = missing()
        .build()
        .profile()
        .name();
}
"#,
        InlayHintsQuery::new("method chain expression type hint skips", "/src/lib.rs"),
        expect![[r#"
            method chain expression type hint skips
            - <none>
        "#]],
    );
}

#[test]
fn shows_closing_brace_hints_for_long_named_blocks() {
    check_inlay_hints(
        r#"
//- /Cargo.toml
[package]
name = "analysis_closing_brace_named_hints"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod outer {
    pub struct User;

    impl User {
        pub fn process(&self) {
            self;
            self;
            self;
            self;
            self;
            self;
            self;
            self;
            self;
            self;
            self;
            self;
            self;
            self;
            self;
            self;
            self;
            self;
            self;
            self;
        }
    }
}
"#,
        InlayHintsQuery::new("closing brace named hints", "/src/lib.rs"),
        expect![[r#"
            closing brace named hints
            - `// fn process` @ 26:9-26:10
            - `// impl User` @ 27:5-27:6
            - `// mod outer` @ 28:1-28:2
        "#]],
    );
}

#[test]
fn shows_closing_brace_hints_for_long_control_flow_blocks() {
    check_inlay_hints(
        r#"
//- /Cargo.toml
[package]
name = "analysis_closing_brace_flow_hints"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Mode {
    Fast,
    Slow,
}

pub fn process(mode: Mode) {
    match mode {
        Mode::Fast => {
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
        }
        Mode::Slow => {}
    }

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
}
"#,
        InlayHintsQuery::new("closing brace control-flow hints", "/src/lib.rs"),
        expect![[r#"
            closing brace control-flow hints
            - `// match mode` @ 30:5-30:6
            - `// loop` @ 53:5-53:6
            - `// fn process` @ 54:1-54:2
        "#]],
    );
}

#[test]
fn shows_closing_brace_hints_for_loop_conditions() {
    check_inlay_hints(
        r#"
//- /Cargo.toml
[package]
name = "analysis_closing_brace_loop_detail_hints"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Items;

pub fn process(items: Items, active: bool) {
    for item in items {
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
        ();
    }

    while active {
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
}
"#,
        InlayHintsQuery::new("closing brace loop detail hints", "/src/lib.rs"),
        expect![[r#"
            closing brace loop detail hints
            - `// for item in items` @ 25:5-25:6
            - `// while active` @ 48:5-48:6
            - `// fn process` @ 49:1-49:2
        "#]],
    );
}

#[test]
fn skips_closing_brace_hints_for_short_blocks() {
    check_inlay_hints(
        r#"
//- /Cargo.toml
[package]
name = "analysis_short_closing_brace_hints"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn short() {
    ();
}
"#,
        InlayHintsQuery::new("short closing brace hints", "/src/lib.rs"),
        expect![[r#"
            short closing brace hints
            - <none>
        "#]],
    );
}

#[test]
fn shows_parameter_hints_for_resolved_calls() {
    check_inlay_hints(
        r#"
//- /Cargo.toml
[package]
name = "analysis_parameter_hints"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn build(scope: u32, annotation: User, initializer: User) -> User {
    initializer
}

impl User {
    pub fn update(&self, active: bool, pending_tys: User) {}

    pub fn make(value: User, count: u32) -> User {
        value
    }
}

pub fn use_it(scope: u32, user: User, other: User) {
    build(scope, user, other);
    user.update(true, other);
    User::make(user, 10);
}
"#,
        InlayHintsQuery::new("parameter hints", "/src/lib.rs"),
        expect![[r#"
            parameter hints
            - `annotation:` @ 16:18-16:22
            - `initializer:` @ 16:24-16:29
            - `active:` @ 17:17-17:21
            - `pending_tys:` @ 17:23-17:28
            - `value:` @ 18:16-18:20
            - `count:` @ 18:22-18:24
        "#]],
    );
}

#[test]
fn skips_noisy_or_unresolved_parameter_hints() {
    check_inlay_hints(
        r#"
//- /Cargo.toml
[package]
name = "analysis_parameter_hint_skips"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn destructured((left, right): (User, User), _: User, normal: User) {}

pub fn use_it(user: User, other: User, normal: User) {
    destructured((user, other), user, normal);
    missing(user);
}
"#,
        InlayHintsQuery::new("parameter hint skips", "/src/lib.rs"),
        expect![[r#"
            parameter hint skips
            - <none>
        "#]],
    );
}
