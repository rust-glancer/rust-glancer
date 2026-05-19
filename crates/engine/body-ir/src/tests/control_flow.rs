use expect_test::expect;

use super::utils::check_project_body_ir;

#[test]
fn lowers_if_let_scope_and_branch_types() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_if_let_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct UserId(u64);

impl UserId {
    pub fn is_valid(&self) -> bool {
        true
    }
}

pub enum Maybe {
    Some(UserId),
    None,
}

pub fn choose(input: Maybe, fallback: UserId) -> UserId {
    if let Maybe::Some(id) = input && id.is_valid() {
        id
    } else {
        fallback
    }
}
"#,
        expect![[r#"
            package body_if_let_fixture

            body_if_let_fixture [lib]
            body b0 fn body_if_let_fixture[lib]::crate::choose @ 14:1-20:2
            scopes
            - s0 parent <none>: v0, v1
            - s1 parent s0: <none>
            - s2 parent s1: v2
            - s3 parent s2: <none>
            - s4 parent s1: <none>
            bindings
            - v0 param input `input`: Maybe => nominal enum body_if_let_fixture[lib]::crate::Maybe @ 14:15-14:20
            - v1 param fallback `fallback`: UserId => nominal struct body_if_let_fixture[lib]::crate::UserId @ 14:29-14:37
            - v2 let id `id` => nominal struct body_if_let_fixture[lib]::crate::UserId @ 15:24-15:26
            body
            expr e10 block s1 => nominal struct body_if_let_fixture[lib]::crate::UserId @ 14:57-20:2
              tail
                expr e9 if => nominal struct body_if_let_fixture[lib]::crate::UserId @ 15:5-19:6
                  condition
                    expr e4 binary && => <unknown> @ 15:8-15:52
                      lhs
                        expr e1 let s2 v2 => <unknown> @ 15:8-15:35
                          initializer
                            expr e0 path input -> local v0 => nominal enum body_if_let_fixture[lib]::crate::Maybe @ 15:30-15:35
                      rhs
                        expr e3 method_call is_valid -> fn impl UserId::is_valid => syntax bool @ 15:39-15:52
                          receiver
                            expr e2 path id -> local v2 => nominal struct body_if_let_fixture[lib]::crate::UserId @ 15:39-15:41
                  then
                    expr e6 block s3 => nominal struct body_if_let_fixture[lib]::crate::UserId @ 15:53-17:6
                      tail
                        expr e5 path id -> local v2 => nominal struct body_if_let_fixture[lib]::crate::UserId @ 16:9-16:11
                  else
                    expr e8 block s4 => nominal struct body_if_let_fixture[lib]::crate::UserId @ 17:12-19:6
                      tail
                        expr e7 path fallback -> local v1 => nominal struct body_if_let_fixture[lib]::crate::UserId @ 18:9-18:17


            body b1 fn impl UserId::is_valid @ 4:5-6:6
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `&self` => Self struct body_if_let_fixture[lib]::crate::UserId @ 4:21-4:26
            body
            expr e1 block s1 => <unknown> @ 4:36-6:6
              tail
                expr e0 literal bool `true` => <unknown> @ 5:9-5:13
        "#]],
    );
}

#[test]
fn lowers_let_else_and_match_guards() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_let_else_guard_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct UserId(u64);

impl UserId {
    pub fn is_valid(&self) -> bool {
        true
    }
}

pub enum Maybe {
    Some(UserId),
    None,
}

pub fn choose(input: Maybe, fallback: UserId) -> UserId {
    let Maybe::Some(fallback) = input else {
        return fallback;
    };

    match input {
        Maybe::Some(user) if user.is_valid() => user,
        Maybe::None => fallback,
    }
}
"#,
        expect![[r#"
            package body_let_else_guard_fixture

            body_let_else_guard_fixture [lib]
            body b0 fn body_let_else_guard_fixture[lib]::crate::choose @ 14:1-23:2
            scopes
            - s0 parent <none>: v0, v1
            - s1 parent s0: v2
            - s2 parent s1: <none>
            - s3 parent s1: v3
            - s4 parent s1: <none>
            bindings
            - v0 param input `input`: Maybe => nominal enum body_let_else_guard_fixture[lib]::crate::Maybe @ 14:15-14:20
            - v1 param fallback `fallback`: UserId => nominal struct body_let_else_guard_fixture[lib]::crate::UserId @ 14:29-14:37
            - v2 let fallback `fallback` => nominal struct body_let_else_guard_fixture[lib]::crate::UserId @ 15:21-15:29
            - v3 let user `user` => nominal struct body_let_else_guard_fixture[lib]::crate::UserId @ 20:21-20:25
            body
            expr e10 block s1 => nominal struct body_let_else_guard_fixture[lib]::crate::UserId @ 14:57-23:2
              stmt s1 let v2 @ 15:5-17:7
                initializer
                  expr e0 path input -> local v0 => nominal enum body_let_else_guard_fixture[lib]::crate::Maybe @ 15:33-15:38
                else
                  expr e3 block s2 => () @ 15:44-17:6
                    stmt s0 expr; @ 16:9-16:25
                      expr e2 wrapper return => ! @ 16:9-16:24
                        inner
                          expr e1 path fallback -> local v1 => nominal struct body_let_else_guard_fixture[lib]::crate::UserId @ 16:16-16:24
              tail
                expr e9 match => nominal struct body_let_else_guard_fixture[lib]::crate::UserId @ 19:5-22:6
                  scrutinee
                    expr e4 path input -> local v0 => nominal enum body_let_else_guard_fixture[lib]::crate::Maybe @ 19:11-19:16
                  arm s3
                    guard
                      expr e6 method_call is_valid -> fn impl UserId::is_valid => syntax bool @ 20:30-20:45
                        receiver
                          expr e5 path user -> local v3 => nominal struct body_let_else_guard_fixture[lib]::crate::UserId @ 20:30-20:34
                    expr e7 path user -> local v3 => nominal struct body_let_else_guard_fixture[lib]::crate::UserId @ 20:49-20:53
                  arm s4
                    expr e8 path fallback -> local v2 => nominal struct body_let_else_guard_fixture[lib]::crate::UserId @ 21:24-21:32


            body b1 fn impl UserId::is_valid @ 4:5-6:6
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `&self` => Self struct body_let_else_guard_fixture[lib]::crate::UserId @ 4:21-4:26
            body
            expr e1 block s1 => <unknown> @ 4:36-6:6
              tail
                expr e0 literal bool `true` => <unknown> @ 5:9-5:13
        "#]],
    );
}

#[test]
fn lowers_loop_while_for_and_jumps() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_control_flow_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct UserId(u64);
pub struct Items;

pub enum Maybe {
    Some(UserId),
    None,
}

pub fn walk(input: Maybe, items: Items) {
    'outer: loop {
        while let Maybe::Some(item) = input {
            item;
            break 'outer item;
        }
        for value in items {
            value;
            continue 'outer;
        }
        break;
    }
}
"#,
        expect![[r#"
            package body_control_flow_fixture

            body_control_flow_fixture [lib]
            body b0 fn body_control_flow_fixture[lib]::crate::walk @ 9:1-21:2
            scopes
            - s0 parent <none>: v0, v1
            - s1 parent s0: <none>
            - s2 parent s1: <none>
            - s3 parent s2: v2
            - s4 parent s3: <none>
            - s5 parent s2: v3
            - s6 parent s5: <none>
            bindings
            - v0 param input `input`: Maybe => nominal enum body_control_flow_fixture[lib]::crate::Maybe @ 9:13-9:18
            - v1 param items `items`: Items => nominal struct body_control_flow_fixture[lib]::crate::Items @ 9:27-9:32
            - v2 let item `item` => nominal struct body_control_flow_fixture[lib]::crate::UserId @ 11:31-11:35
            - v3 let value `value` => <unknown> @ 15:13-15:18
            body
            expr e15 block s1 => <unknown> @ 9:41-21:2
              tail
                expr e14 loop 'outer => <unknown> @ 10:5-20:6
                  body
                    expr e13 block s2 => () @ 10:18-20:6
                      stmt s2 expr @ 11:9-14:10
                        expr e6 while => () @ 11:9-14:10
                          condition
                            expr e1 let s3 v2 => <unknown> @ 11:15-11:44
                              initializer
                                expr e0 path input -> local v0 => nominal enum body_control_flow_fixture[lib]::crate::Maybe @ 11:39-11:44
                          body
                            expr e5 block s4 => () @ 11:45-14:10
                              stmt s0 expr; @ 12:13-12:18
                                expr e2 path item -> local v2 => nominal struct body_control_flow_fixture[lib]::crate::UserId @ 12:13-12:17
                              stmt s1 expr; @ 13:13-13:31
                                expr e4 break 'outer => ! @ 13:13-13:30
                                  value
                                    expr e3 path item -> local v2 => nominal struct body_control_flow_fixture[lib]::crate::UserId @ 13:26-13:30
                      stmt s5 expr @ 15:9-18:10
                        expr e11 for s5 v3 => () @ 15:9-18:10
                          iterable
                            expr e7 path items -> local v1 => nominal struct body_control_flow_fixture[lib]::crate::Items @ 15:22-15:27
                          body
                            expr e10 block s6 => () @ 15:28-18:10
                              stmt s3 expr; @ 16:13-16:19
                                expr e8 path value -> local v3 => <unknown> @ 16:13-16:18
                              stmt s4 expr; @ 17:13-17:29
                                expr e9 continue 'outer => ! @ 17:13-17:28
                      stmt s6 expr; @ 19:9-19:15
                        expr e12 break => ! @ 19:9-19:14
        "#]],
    );
}

#[test]
fn lowers_labeled_block_control_flow() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_labeled_block_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct UserId(u64);

pub fn choose(value: UserId) -> UserId {
    'done: {
        break 'done value;
    }
}
"#,
        expect![[r#"
            package body_labeled_block_fixture

            body_labeled_block_fixture [lib]
            body b0 fn body_labeled_block_fixture[lib]::crate::choose @ 3:1-7:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            - s2 parent s1: <none>
            bindings
            - v0 param value `value`: UserId => nominal struct body_labeled_block_fixture[lib]::crate::UserId @ 3:15-3:20
            body
            expr e3 block s1 => () @ 3:40-7:2
              tail
                expr e2 block 'done s2 => () @ 4:5-6:6
                  stmt s0 expr; @ 5:9-5:27
                    expr e1 break 'done => ! @ 5:9-5:26
                      value
                        expr e0 path value -> local v0 => nominal struct body_labeled_block_fixture[lib]::crate::UserId @ 5:21-5:26
        "#]],
    );
}
