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
                    expr e4 binary && => bool @ 15:8-15:52
                      lhs
                        expr e1 let s2 v2 => bool @ 15:8-15:35
                          initializer
                            expr e0 path input -> local v0 => nominal enum body_if_let_fixture[lib]::crate::Maybe @ 15:30-15:35
                      rhs
                        expr e3 method_call is_valid -> fn impl UserId::is_valid => bool @ 15:39-15:52
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
            - v0 self_param self `&self` => &Self struct body_if_let_fixture[lib]::crate::UserId @ 4:21-4:26
            body
            expr e1 block s1 => bool @ 4:36-6:6
              tail
                expr e0 literal bool `true` => bool @ 5:9-5:13
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
                  expr e3 block s2 => ! @ 15:44-17:6
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
                      expr e6 method_call is_valid -> fn impl UserId::is_valid => bool @ 20:30-20:45
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
            - v0 self_param self `&self` => &Self struct body_let_else_guard_fixture[lib]::crate::UserId @ 4:21-4:26
            body
            expr e1 block s1 => bool @ 4:36-6:6
              tail
                expr e0 literal bool `true` => bool @ 5:9-5:13
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
                    expr e13 block s2 => ! @ 10:18-20:6
                      stmt s2 expr @ 11:9-14:10
                        expr e6 while => () @ 11:9-14:10
                          condition
                            expr e1 let s3 v2 => bool @ 11:15-11:44
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
                            expr e10 block s6 => ! @ 15:28-18:10
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
fn propagates_for_loop_items_from_into_iterator() {
    check_project_body_ir(
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
        expect![[r#"
            package app

            app [lib]
            body b0 fn app[lib]::crate::use_it @ 4:1-17:2
            scopes
            - s0 parent <none>: v0, v1, v2
            - s1 parent s0: <none>
            - s2 parent s1: v3
            - s3 parent s2: <none>
            - s4 parent s1: v4
            - s5 parent s4: <none>
            - s6 parent s1: v5, v6
            - s7 parent s6: <none>
            bindings
            - v0 param packages `packages`: &[Package] => &[nominal struct app[lib]::crate::Package] @ 4:15-4:23
            - v1 param array `array`: [Package; 3] => [nominal struct app[lib]::crate::Package; 3] @ 4:37-4:42
            - v2 param pairs `pairs`: [(Package, UserId); 2] => [(nominal struct app[lib]::crate::Package, nominal struct app[lib]::crate::UserId); 2] @ 4:58-4:63
            - v3 let borrowed `borrowed` => &nominal struct app[lib]::crate::Package @ 5:9-5:17
            - v4 let owned `owned` => nominal struct app[lib]::crate::Package @ 9:9-9:14
            - v5 let package `package` => nominal struct app[lib]::crate::Package @ 13:10-13:17
            - v6 let user_id `user_id` => nominal struct app[lib]::crate::UserId @ 13:19-13:26
            body
            expr e13 block s1 => () @ 4:89-17:2
              stmt s1 expr @ 5:5-7:6
                expr e3 for s2 v3 => () @ 5:5-7:6
                  iterable
                    expr e0 path packages -> local v0 => &[nominal struct app[lib]::crate::Package] @ 5:21-5:29
                  body
                    expr e2 block s3 => () @ 5:30-7:6
                      stmt s0 expr; @ 6:9-6:18
                        expr e1 path borrowed -> local v3 => &nominal struct app[lib]::crate::Package @ 6:9-6:17
              stmt s3 expr @ 9:5-11:6
                expr e7 for s4 v4 => () @ 9:5-11:6
                  iterable
                    expr e4 path array -> local v1 => [nominal struct app[lib]::crate::Package; 3] @ 9:18-9:23
                  body
                    expr e6 block s5 => () @ 9:24-11:6
                      stmt s2 expr; @ 10:9-10:15
                        expr e5 path owned -> local v4 => nominal struct app[lib]::crate::Package @ 10:9-10:14
              tail
                expr e12 for s6 v5, v6 => () @ 13:5-16:6
                  iterable
                    expr e8 path pairs -> local v2 => [(nominal struct app[lib]::crate::Package, nominal struct app[lib]::crate::UserId); 2] @ 13:31-13:36
                  body
                    expr e11 block s7 => () @ 13:37-16:6
                      stmt s4 expr; @ 14:9-14:17
                        expr e9 path package -> local v5 => nominal struct app[lib]::crate::Package @ 14:9-14:16
                      stmt s5 expr; @ 15:9-15:17
                        expr e10 path user_id -> local v6 => nominal struct app[lib]::crate::UserId @ 15:9-15:16


            package fake_core

            fake_core [lib]
        "#]],
    );
}

#[test]
fn propagates_for_loop_items_from_method_returned_slice() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[workspace]
members = ["core", "storage", "app"]
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

//- /storage/Cargo.toml
[package]
name = "storage"
version = "0.1.0"
edition = "2024"

//- /storage/src/lib.rs
pub struct ImportData;

pub struct DefMap;

impl DefMap {
    pub fn imports(&self) -> &[ImportData] {
        missing()
    }
}

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
core = { package = "fake_core", path = "../core" }
storage = { path = "../storage" }

//- /app/src/lib.rs
use storage::DefMap;

pub fn use_it(def_map: &DefMap) {
    for import in def_map.imports() {
        import;
    }
}
"#,
        expect![[r#"
            package app

            app [lib]
            body b0 fn app[lib]::crate::use_it @ 3:1-7:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            - s2 parent s1: v1
            - s3 parent s2: <none>
            bindings
            - v0 param def_map `def_map`: &DefMap => &nominal struct storage[lib]::crate::DefMap @ 3:15-3:22
            - v1 let import `import` => &nominal struct storage[lib]::crate::ImportData @ 4:9-4:15
            body
            expr e5 block s1 => () @ 3:33-7:2
              tail
                expr e4 for s2 v1 => () @ 4:5-6:6
                  iterable
                    expr e1 method_call imports -> fn impl DefMap::imports => &[nominal struct storage[lib]::crate::ImportData] @ 4:19-4:36
                      receiver
                        expr e0 path def_map -> local v0 => &nominal struct storage[lib]::crate::DefMap @ 4:19-4:26
                  body
                    expr e3 block s3 => () @ 4:37-6:6
                      stmt s0 expr; @ 5:9-5:16
                        expr e2 path import -> local v1 => &nominal struct storage[lib]::crate::ImportData @ 5:9-5:15


            package fake_core

            fake_core [lib]

            package storage

            storage [lib]
            body b0 fn impl DefMap::imports @ 6:5-8:6
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `&self` => &Self struct storage[lib]::crate::DefMap @ 6:20-6:25
            body
            expr e2 block s1 => <unknown> @ 6:44-8:6
              tail
                expr e1 call => <unknown> @ 7:9-7:18
                  callee
                    expr e0 path missing => <unknown> @ 7:9-7:16
        "#]],
    );
}

#[test]
fn propagates_for_loop_items_from_slice_iter_method() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[workspace]
members = ["core", "storage", "app"]
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

    pub trait Iterator {
        type Item;
    }
}

pub mod slice {
    pub struct Iter<'a, T>(&'a T);
}

impl<T> [T] {
    pub fn iter(&self) -> slice::Iter<'_, T> {
        missing()
    }
}

impl<'a, T> iter::Iterator for slice::Iter<'a, T> {
    type Item = &'a T;
}

impl<I: iter::Iterator> iter::IntoIterator for I {
    type Item = I::Item;
}

//- /storage/Cargo.toml
[package]
name = "storage"
version = "0.1.0"
edition = "2024"

//- /storage/src/lib.rs
pub struct ImportData;

pub struct DefMap;

impl DefMap {
    pub fn imports(&self) -> &[ImportData] {
        missing()
    }
}

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
core = { package = "fake_core", path = "../core" }
storage = { path = "../storage" }

//- /app/src/lib.rs
use storage::DefMap;

pub fn use_it(def_map: &DefMap) {
    for import in def_map.imports().iter() {
        import;
    }
}
"#,
        expect![[r#"
            package app

            app [lib]
            body b0 fn app[lib]::crate::use_it @ 3:1-7:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            - s2 parent s1: v1
            - s3 parent s2: <none>
            bindings
            - v0 param def_map `def_map`: &DefMap => &nominal struct storage[lib]::crate::DefMap @ 3:15-3:22
            - v1 let import `import` => &nominal struct storage[lib]::crate::ImportData @ 4:9-4:15
            body
            expr e6 block s1 => () @ 3:33-7:2
              tail
                expr e5 for s2 v1 => () @ 4:5-6:6
                  iterable
                    expr e2 method_call iter -> fn impl [T]::iter => nominal struct fake_core[lib]::crate::slice::Iter<'_, nominal struct storage[lib]::crate::ImportData> @ 4:19-4:43
                      receiver
                        expr e1 method_call imports -> fn impl DefMap::imports => &[nominal struct storage[lib]::crate::ImportData] @ 4:19-4:36
                          receiver
                            expr e0 path def_map -> local v0 => &nominal struct storage[lib]::crate::DefMap @ 4:19-4:26
                  body
                    expr e4 block s3 => () @ 4:44-6:6
                      stmt s0 expr; @ 5:9-5:16
                        expr e3 path import -> local v1 => &nominal struct storage[lib]::crate::ImportData @ 5:9-5:15


            package fake_core

            fake_core [lib]
            body b0 fn impl [T]::iter @ 16:5-18:6
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `&self` => <unknown> @ 16:17-16:22
            body
            expr e2 block s1 => <unknown> @ 16:46-18:6
              tail
                expr e1 call => <unknown> @ 17:9-17:18
                  callee
                    expr e0 path missing => <unknown> @ 17:9-17:16


            package storage

            storage [lib]
            body b0 fn impl DefMap::imports @ 6:5-8:6
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `&self` => &Self struct storage[lib]::crate::DefMap @ 6:20-6:25
            body
            expr e2 block s1 => <unknown> @ 6:44-8:6
              tail
                expr e1 call => <unknown> @ 7:9-7:18
                  callee
                    expr e0 path missing => <unknown> @ 7:9-7:16
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
