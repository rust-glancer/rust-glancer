use expect_test::expect;

use super::utils::check_project_body_ir;

#[test]
fn propagates_enum_variant_payload_types_through_patterns() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_enum_pattern_fixture"
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
    match maybe {
        Some(user) => user,
        None => value,
    }
}
"#,
        expect![[r#"
            package body_enum_pattern_fixture

            body_enum_pattern_fixture [lib]
            body b0 fn body_enum_pattern_fixture[lib]::crate::use_it @ 8:1-14:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1
            - s2 parent s1: v2
            - s3 parent s1: <none>
            bindings
            - v0 param maybe `maybe`: Option<User> => nominal enum body_enum_pattern_fixture[lib]::crate::Option<nominal struct body_enum_pattern_fixture[lib]::crate::User> @ 8:15-8:20
            - v1 let value `value` => nominal struct body_enum_pattern_fixture[lib]::crate::User @ 9:14-9:19
            - v2 let user `user` => nominal struct body_enum_pattern_fixture[lib]::crate::User @ 11:14-11:18
            body
            expr e5 block s1 => nominal struct body_enum_pattern_fixture[lib]::crate::User @ 8:36-14:2
              stmt s0 let v1 @ 9:5-9:46
                initializer
                  expr e0 path maybe -> local v0 => nominal enum body_enum_pattern_fixture[lib]::crate::Option<nominal struct body_enum_pattern_fixture[lib]::crate::User> @ 9:23-9:28
              tail
                expr e4 match => nominal struct body_enum_pattern_fixture[lib]::crate::User @ 10:5-13:6
                  scrutinee
                    expr e1 path maybe -> local v0 => nominal enum body_enum_pattern_fixture[lib]::crate::Option<nominal struct body_enum_pattern_fixture[lib]::crate::User> @ 10:11-10:16
                  arm s2
                    expr e2 path user -> local v2 => nominal struct body_enum_pattern_fixture[lib]::crate::User @ 11:23-11:27
                  arm s3
                    expr e3 path value -> local v1 => nominal struct body_enum_pattern_fixture[lib]::crate::User @ 12:17-12:22
        "#]],
    );
}

#[test]
fn collects_bindings_from_destructuring_patterns() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_destructure_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct UserId(u64);

pub struct Pair {
    pub left: UserId,
    pub right: UserId,
}

pub fn destructure(
    (param_left, param_right): (UserId, UserId),
    pair: (UserId, UserId),
    record: Pair,
    borrowed: &(UserId, UserId),
) -> UserId {
    let from_param: UserId = param_left;
    let (left, right) = pair;
    let Pair { left: field_left, right } = record;
    let &(borrowed_left, borrowed_right) = borrowed;
    left
}
"#,
        expect![[r#"
            package body_destructure_fixture

            body_destructure_fixture [lib]
            body b0 fn body_destructure_fixture[lib]::crate::destructure @ 8:1-19:2
            scopes
            - s0 parent <none>: v0, v1, v2, v3, v4
            - s1 parent s0: v5, v6, v7, v8, v9, v10, v11
            bindings
            - v0 param param_left `param_left` => <unknown> @ 9:6-9:16
            - v1 param param_right `param_right` => <unknown> @ 9:18-9:29
            - v2 param pair `pair`: (UserId, UserId) => syntax (UserId, UserId) @ 10:5-10:9
            - v3 param record `record`: Pair => nominal struct body_destructure_fixture[lib]::crate::Pair @ 11:5-11:11
            - v4 param borrowed `borrowed`: &(UserId, UserId) => &syntax (UserId, UserId) @ 12:5-12:13
            - v5 let from_param `from_param`: UserId => nominal struct body_destructure_fixture[lib]::crate::UserId @ 14:9-14:19
            - v6 let left `left` => <unknown> @ 15:10-15:14
            - v7 let right `right` => <unknown> @ 15:16-15:21
            - v8 let field_left `field_left` => <unknown> @ 16:22-16:32
            - v9 let right `right` => <unknown> @ 16:34-16:39
            - v10 let borrowed_left `borrowed_left` => <unknown> @ 17:11-17:24
            - v11 let borrowed_right `borrowed_right` => <unknown> @ 17:26-17:40
            body
            expr e5 block s1 => <unknown> @ 13:13-19:2
              stmt s0 let v5: UserId @ 14:5-14:41
                initializer
                  expr e0 path param_left -> local v0 => <unknown> @ 14:30-14:40
              stmt s1 let v6, v7 @ 15:5-15:30
                initializer
                  expr e1 path pair -> local v2 => syntax (UserId, UserId) @ 15:25-15:29
              stmt s2 let v8, v9 @ 16:5-16:51
                initializer
                  expr e2 path record -> local v3 => nominal struct body_destructure_fixture[lib]::crate::Pair @ 16:44-16:50
              stmt s3 let v10, v11 @ 17:5-17:53
                initializer
                  expr e3 path borrowed -> local v4 => &syntax (UserId, UserId) @ 17:44-17:52
              tail
                expr e4 path left -> local v6 => <unknown> @ 18:5-18:9
        "#]],
    );
}
