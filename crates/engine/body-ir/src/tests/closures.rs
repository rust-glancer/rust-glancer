use expect_test::expect;

use super::utils::check_project_body_ir;

#[test]
fn lowers_closure_scopes_params_and_body() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_closure_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn use_it(user: User) -> User {
    let pick = async move |user: User| -> User { user };
    let pair = |(left, right): (User, User)| left;
    user
}
"#,
        expect![[r#"
            package body_closure_fixture

            body_closure_fixture [lib]
            body b0 fn body_closure_fixture[lib]::crate::use_it @ 3:1-7:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v2, v5
            - s2 parent s1: v1
            - s3 parent s2: <none>
            - s4 parent s1: v3, v4
            bindings
            - v0 param user `user`: User => nominal struct body_closure_fixture[lib]::crate::User @ 3:15-3:19
            - v1 param user `user`: User => nominal struct body_closure_fixture[lib]::crate::User @ 4:28-4:32
            - v2 let pick `pick` => <unknown> @ 4:9-4:13
            - v3 param left `left` => <unknown> @ 5:18-5:22
            - v4 param right `right` => <unknown> @ 5:24-5:29
            - v5 let pair `pair` => <unknown> @ 5:9-5:13
            body
            expr e6 block s1 => nominal struct body_closure_fixture[lib]::crate::User @ 3:35-7:2
              stmt s0 let v2 @ 4:5-4:57
                initializer
                  expr e2 closure async move s2 (v1: User) -> User => <unknown> @ 4:16-4:56
                    body
                      expr e1 block s3 => nominal struct body_closure_fixture[lib]::crate::User @ 4:48-4:56
                        tail
                          expr e0 path user -> local v1 => nominal struct body_closure_fixture[lib]::crate::User @ 4:50-4:54
              stmt s1 let v5 @ 5:5-5:51
                initializer
                  expr e4 closure s4 (v3, v4: (User, User)) => <unknown> @ 5:16-5:50
                    body
                      expr e3 path left -> local v3 => <unknown> @ 5:46-5:50
              tail
                expr e5 path user -> local v0 => nominal struct body_closure_fixture[lib]::crate::User @ 6:5-6:9
        "#]],
    );
}

#[test]
fn lowers_unannotated_closure_params_as_scope_bindings() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_untyped_closure_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn use_it(user: User) -> User {
    let echo = |user| user;
    user
}
"#,
        expect![[r#"
            package body_untyped_closure_fixture

            body_untyped_closure_fixture [lib]
            body b0 fn body_untyped_closure_fixture[lib]::crate::use_it @ 3:1-6:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v2
            - s2 parent s1: v1
            bindings
            - v0 param user `user`: User => nominal struct body_untyped_closure_fixture[lib]::crate::User @ 3:15-3:19
            - v1 param user `user` => <unknown> @ 4:17-4:21
            - v2 let echo `echo` => <unknown> @ 4:9-4:13
            body
            expr e3 block s1 => nominal struct body_untyped_closure_fixture[lib]::crate::User @ 3:35-6:2
              stmt s0 let v2 @ 4:5-4:28
                initializer
                  expr e1 closure s2 (v1) => <unknown> @ 4:16-4:27
                    body
                      expr e0 path user -> local v1 => <unknown> @ 4:23-4:27
              tail
                expr e2 path user -> local v0 => nominal struct body_untyped_closure_fixture[lib]::crate::User @ 5:5-5:9
        "#]],
    );
}
