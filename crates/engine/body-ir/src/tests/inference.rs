use expect_test::expect;

use super::utils::check_project_body_ir;

#[test]
fn records_simple_assignment_inference_facts() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_assignment_inference_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn missing<T>() -> T {}
pub fn id<T>(value: T) -> T {}

pub fn use_it(user: User) {
    let mut value = id(missing());
    value = user;
    value;
}
"#,
        expect![[r#"
            package body_assignment_inference_fixture

            body_assignment_inference_fixture [lib]
            body b0 fn body_assignment_inference_fixture[lib]::crate::missing @ 3:1-3:28
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e0 block s1 => () @ 3:26-3:28


            body b1 fn body_assignment_inference_fixture[lib]::crate::id @ 4:1-4:31
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 param value `value`: T => syntax T @ 4:14-4:19
            body
            expr e0 block s1 => () @ 4:29-4:31


            body b2 fn body_assignment_inference_fixture[lib]::crate::use_it @ 6:1-10:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1
            bindings
            - v0 param user `user`: User => nominal struct body_assignment_inference_fixture[lib]::crate::User @ 6:15-6:19
            - v1 let value `mut value` => nominal struct body_assignment_inference_fixture[lib]::crate::User @ 7:9-7:18 name @ 7:13-7:18
            body
            expr e8 block s1 => () @ 6:27-10:2
              stmt s0 let v1 @ 7:5-7:35
                initializer
                  expr e3 call => nominal struct body_assignment_inference_fixture[lib]::crate::User @ 7:21-7:34
                    callee
                      expr e0 path id -> item fn body_assignment_inference_fixture[lib]::crate::id => <unknown> @ 7:21-7:23
                    arg
                      expr e2 call => nominal struct body_assignment_inference_fixture[lib]::crate::User @ 7:24-7:33
                        callee
                          expr e1 path missing -> item fn body_assignment_inference_fixture[lib]::crate::missing => <unknown> @ 7:24-7:31
              stmt s1 expr; @ 8:5-8:18
                expr e6 assign = => () @ 8:5-8:17
                  target
                    expr e4 path value -> local v1 => nominal struct body_assignment_inference_fixture[lib]::crate::User @ 8:5-8:10
                  value
                    expr e5 path user -> local v0 => nominal struct body_assignment_inference_fixture[lib]::crate::User @ 8:13-8:17
              stmt s2 expr; @ 9:5-9:11
                expr e7 path value -> local v1 => nominal struct body_assignment_inference_fixture[lib]::crate::User @ 9:5-9:10
        "#]],
    );
}

#[test]
fn infers_static_associated_function_impl_generics_from_args() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_static_assoc_fn_impl_generic_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub struct Vec<T> {
    value: T,
}

impl<T> Vec<T> {
    pub fn singleton(value: T) -> Self {}
}

pub fn use_it(user: User) {
    let singleton = Vec::singleton(user);
    singleton;
}
"#,
        expect![[r#"
            package body_static_assoc_fn_impl_generic_inference

            body_static_assoc_fn_impl_generic_inference [lib]
            body b0 fn body_static_assoc_fn_impl_generic_inference[lib]::crate::use_it @ 11:1-14:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1
            bindings
            - v0 param user `user`: User => nominal struct body_static_assoc_fn_impl_generic_inference[lib]::crate::User @ 11:15-11:19
            - v1 let singleton `singleton` => nominal struct body_static_assoc_fn_impl_generic_inference[lib]::crate::Vec<nominal struct body_static_assoc_fn_impl_generic_inference[lib]::crate::User> @ 12:9-12:18
            body
            expr e4 block s1 => () @ 11:27-14:2
              stmt s0 let v1 @ 12:5-12:42
                initializer
                  expr e2 call => nominal struct body_static_assoc_fn_impl_generic_inference[lib]::crate::Vec<nominal struct body_static_assoc_fn_impl_generic_inference[lib]::crate::User> @ 12:21-12:41
                    callee
                      expr e0 path Vec::singleton -> fn impl Vec<T>::singleton => <unknown> @ 12:21-12:35
                    arg
                      expr e1 path user -> local v0 => nominal struct body_static_assoc_fn_impl_generic_inference[lib]::crate::User @ 12:36-12:40
              stmt s1 expr; @ 13:5-13:15
                expr e3 path singleton -> local v1 => nominal struct body_static_assoc_fn_impl_generic_inference[lib]::crate::Vec<nominal struct body_static_assoc_fn_impl_generic_inference[lib]::crate::User> @ 13:5-13:14


            body b1 fn impl Vec<T>::singleton @ 8:5-8:42
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 param value `value`: T => syntax T @ 8:22-8:27
            body
            expr e0 block s1 => () @ 8:40-8:42
        "#]],
    );
}

#[test]
fn ignores_never_branches_when_inferring_common_result() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_never_branch_result_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn use_it(flag: bool, user: User) -> User {
    let value = if flag {
        return user;
    } else {
        user
    };
    value
}
"#,
        expect![[r#"
            package body_never_branch_result_inference

            body_never_branch_result_inference [lib]
            body b0 fn body_never_branch_result_inference[lib]::crate::use_it @ 3:1-10:2
            scopes
            - s0 parent <none>: v0, v1
            - s1 parent s0: v2
            - s2 parent s1: <none>
            - s3 parent s1: <none>
            bindings
            - v0 param flag `flag`: bool => bool @ 3:15-3:19
            - v1 param user `user`: User => nominal struct body_never_branch_result_inference[lib]::crate::User @ 3:27-3:31
            - v2 let value `value` => nominal struct body_never_branch_result_inference[lib]::crate::User @ 4:9-4:14
            body
            expr e8 block s1 => nominal struct body_never_branch_result_inference[lib]::crate::User @ 3:47-10:2
              stmt s1 let v2 @ 4:5-8:7
                initializer
                  expr e6 if => nominal struct body_never_branch_result_inference[lib]::crate::User @ 4:17-8:6
                    condition
                      expr e0 path flag -> local v0 => bool @ 4:20-4:24
                    then
                      expr e3 block s2 => ! @ 4:25-6:6
                        stmt s0 expr; @ 5:9-5:21
                          expr e2 wrapper return => ! @ 5:9-5:20
                            inner
                              expr e1 path user -> local v1 => nominal struct body_never_branch_result_inference[lib]::crate::User @ 5:16-5:20
                    else
                      expr e5 block s3 => nominal struct body_never_branch_result_inference[lib]::crate::User @ 6:12-8:6
                        tail
                          expr e4 path user -> local v1 => nominal struct body_never_branch_result_inference[lib]::crate::User @ 7:9-7:13
              tail
                expr e7 path value -> local v2 => nominal struct body_never_branch_result_inference[lib]::crate::User @ 9:5-9:10
        "#]],
    );
}
