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
            - v2 let pick `pick` => closure #2 @ 4:9-4:13
            - v3 param left `left` => nominal struct body_closure_fixture[lib]::crate::User @ 5:18-5:22
            - v4 param right `right` => nominal struct body_closure_fixture[lib]::crate::User @ 5:24-5:29
            - v5 let pair `pair` => closure #4 @ 5:9-5:13
            body
            expr e6 block s1 => nominal struct body_closure_fixture[lib]::crate::User @ 3:35-7:2
              stmt s0 let v2 @ 4:5-4:57
                initializer
                  expr e2 closure async move s2 (v1: User) -> User => closure #2 @ 4:16-4:56
                    body
                      expr e1 block s3 => nominal struct body_closure_fixture[lib]::crate::User @ 4:48-4:56
                        tail
                          expr e0 path user -> local v1 => nominal struct body_closure_fixture[lib]::crate::User @ 4:50-4:54
              stmt s1 let v5 @ 5:5-5:51
                initializer
                  expr e4 closure s4 (v3, v4: (User, User)) => closure #4 @ 5:16-5:50
                    body
                      expr e3 path left -> local v3 => nominal struct body_closure_fixture[lib]::crate::User @ 5:46-5:50
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
            - v2 let echo `echo` => closure #1 @ 4:9-4:13
            body
            expr e3 block s1 => nominal struct body_untyped_closure_fixture[lib]::crate::User @ 3:35-6:2
              stmt s0 let v2 @ 4:5-4:28
                initializer
                  expr e1 closure s2 (v1) => closure #1 @ 4:16-4:27
                    body
                      expr e0 path user -> local v1 => <unknown> @ 4:23-4:27
              tail
                expr e2 path user -> local v0 => nominal struct body_untyped_closure_fixture[lib]::crate::User @ 5:5-5:9
        "#]],
    );
}

#[test]
fn infers_closure_params_from_direct_fn_trait_expectations() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_direct_closure_expectation_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Attr;

pub struct AttrVec;

impl AttrVec {
    pub fn push(&mut self, attr: Attr) {}
}

pub struct User;

pub fn with_attrs(f: impl FnOnce(&mut AttrVec)) {}
pub fn with_pair(f: impl FnOnce((User, User)) -> User) {}

pub fn use_it(attr: Attr) {
    with_attrs(|attrs| attrs.push(attr));
    with_pair(|(left, right)| left);
}
"#,
        expect![[r#"
            package body_direct_closure_expectation_fixture

            body_direct_closure_expectation_fixture [lib]
            body b0 fn body_direct_closure_expectation_fixture[lib]::crate::with_attrs @ 11:1-11:51
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 param f `f`: impl FnOnce(&mut AttrVec) => syntax impl FnOnce(&mut AttrVec) @ 11:19-11:20
            body
            expr e0 block s1 => () @ 11:49-11:51


            body b1 fn body_direct_closure_expectation_fixture[lib]::crate::with_pair @ 12:1-12:58
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 param f `f`: impl FnOnce((User, User)) -> User => syntax impl FnOnce((User, User)) -> User @ 12:18-12:19
            body
            expr e0 block s1 => () @ 12:56-12:58


            body b2 fn body_direct_closure_expectation_fixture[lib]::crate::use_it @ 14:1-17:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            - s2 parent s1: v1
            - s3 parent s1: v2, v3
            bindings
            - v0 param attr `attr`: Attr => nominal struct body_direct_closure_expectation_fixture[lib]::crate::Attr @ 14:15-14:19
            - v1 param attrs `attrs` => &mut nominal struct body_direct_closure_expectation_fixture[lib]::crate::AttrVec @ 15:17-15:22
            - v2 param left `left` => nominal struct body_direct_closure_expectation_fixture[lib]::crate::User @ 16:17-16:21
            - v3 param right `right` => nominal struct body_direct_closure_expectation_fixture[lib]::crate::User @ 16:23-16:28
            body
            expr e10 block s1 => () @ 14:27-17:2
              stmt s0 expr; @ 15:5-15:42
                expr e5 call => () @ 15:5-15:41
                  callee
                    expr e0 path with_attrs -> fn body_direct_closure_expectation_fixture[lib]::crate::with_attrs => <unknown> @ 15:5-15:15
                  arg
                    expr e4 closure s2 (v1) => closure #4 @ 15:16-15:40
                      body
                        expr e3 method_call push -> fn impl AttrVec::push => () @ 15:24-15:40
                          receiver
                            expr e1 path attrs -> local v1 => &mut nominal struct body_direct_closure_expectation_fixture[lib]::crate::AttrVec @ 15:24-15:29
                          arg
                            expr e2 path attr -> local v0 => nominal struct body_direct_closure_expectation_fixture[lib]::crate::Attr @ 15:35-15:39
              stmt s1 expr; @ 16:5-16:37
                expr e9 call => () @ 16:5-16:36
                  callee
                    expr e6 path with_pair -> fn body_direct_closure_expectation_fixture[lib]::crate::with_pair => <unknown> @ 16:5-16:14
                  arg
                    expr e8 closure s3 (v2, v3) => closure #8 @ 16:15-16:35
                      body
                        expr e7 path left -> local v2 => nominal struct body_direct_closure_expectation_fixture[lib]::crate::User @ 16:31-16:35


            body b3 fn impl AttrVec::push @ 6:5-6:42
            scopes
            - s0 parent <none>: v0, v1
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `&mut self` => &mut Self struct body_direct_closure_expectation_fixture[lib]::crate::AttrVec @ 6:17-6:26
            - v1 param attr `attr`: Attr => nominal struct body_direct_closure_expectation_fixture[lib]::crate::Attr @ 6:28-6:32
            body
            expr e0 block s1 => () @ 6:40-6:42
        "#]],
    );
}
