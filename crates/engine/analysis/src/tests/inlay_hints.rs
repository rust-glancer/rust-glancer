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
