use expect_test::expect;

use super::utils::{TypeHintsQuery, check_type_hints};

#[test]
fn shows_inferred_local_binding_types() {
    check_type_hints(
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
        TypeHintsQuery::new("type hints", "/src/lib.rs"),
        expect![[r#"
            type hints
            - `: User` @ 11:9-11:13
            - `: User` @ 14:9-14:14
        "#]],
    );
}

#[test]
fn shows_type_hints_inside_bin_roots() {
    check_type_hints(
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
        TypeHintsQuery::new("bin type hints", "/src/main.rs").in_bin("analysis_bin_type_hints"),
        expect![[r#"
            bin type hints
            - `: User` @ 8:9-8:13
        "#]],
    );
}
