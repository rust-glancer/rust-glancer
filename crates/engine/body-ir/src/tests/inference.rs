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
