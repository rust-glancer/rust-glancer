use expect_test::expect;

use super::utils::{check_project_body_ir, check_project_body_ir_with_sysroot};

#[test]
fn expands_module_visible_macro_expression_bodies() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_expr_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_value {
    ($value:expr) => {
        $value + $value
    };
}

pub mod inner {
    pub fn use_it(input: i32) -> i32 {
        let value = make_value!(input);
        value
    }
}
"#,
        expect![[r#"
            package body_expr_macro_fixture

            body_expr_macro_fixture [lib]
            body b0 fn body_expr_macro_fixture[lib]::crate::inner::use_it @ 8:5-11:6
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1
            bindings
            - v0 param input `input`: i32 => i32 @ 8:19-8:24
            - v1 let value `value` => i32 @ 9:13-9:18
            body
            expr e4 block s1 => i32 @ 8:38-11:6
              stmt s0 let v1 @ 9:9-9:40
                initializer
                  expr e2 binary + => i32 @ 9:21-9:39
                    lhs
                      expr e0 path input -> local v0 => i32 @ 9:21-9:39
                    rhs
                      expr e1 path input -> local v0 => i32 @ 9:33-9:38
              tail
                expr e3 path value -> local v1 => i32 @ 10:9-10:14
        "#]],
    );
}

#[test]
fn expands_standard_prelude_macro_expression_bodies() {
    check_project_body_ir_with_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "body_prelude_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() -> u8 {
    make_expr!()
}

//- /sysroot/library/core/src/lib.rs
pub struct Core;

//- /sysroot/library/alloc/src/lib.rs
pub struct Alloc;

//- /sysroot/library/std/src/lib.rs
pub mod macros {
    macro_rules! make_expr {
        () => {
            12u8
        };
    }

    pub use make_expr;
}

pub mod prelude {
    pub mod rust_2024 {
        pub use crate::macros::make_expr;
    }
}
"#,
        expect![[r#"
            package alloc

            alloc [lib]
            skipped

            package body_prelude_macro_fixture

            body_prelude_macro_fixture [lib]
            body b0 fn body_prelude_macro_fixture[lib]::crate::use_it @ 1:1-3:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e1 block s1 => u8 @ 1:23-3:2
              tail
                expr e0 literal int `make_expr!()` => u8 @ 2:5-2:17


            package core

            core [lib]
            skipped

            package std

            std [lib]
            skipped
        "#]],
    );
}

#[test]
fn stops_recursive_macro_expression_expansion() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_expr_recursive_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! recurse {
    () => {
        recurse!()
    };
}

pub fn use_it() {
    let value = recurse!();
}
"#,
        expect![[r#"
            package body_expr_recursive_macro_fixture

            body_expr_recursive_macro_fixture [lib]
            body b0 fn body_expr_recursive_macro_fixture[lib]::crate::use_it @ 7:1-9:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0
            bindings
            - v0 let value `value` => <unknown> @ 8:9-8:14
            body
            expr e1 block s1 => () @ 7:17-9:2
              stmt s0 let v0 @ 8:5-8:28
                initializer
                  expr e0 unknown `recurse!()` => <unknown> @ 8:17-8:27
        "#]],
    );
}

#[test]
fn statement_macros_expand_statements_and_body_local_items() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_stmt_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

macro_rules! make_steps {
    ($input:expr) => {
        let doubled = $input + $input;
        let tripled = doubled + $input;
    };
}

macro_rules! nothing {
    () => {};
}

macro_rules! make_user {
    () => {
        struct User;
    };
}

macro_rules! import_id {
    () => {
        use crate::GlobalId as Id;
    };
}

pub fn steps(input: i32) -> i32 {
    make_steps!(input);
    tripled
}

pub fn empty(input: i32) -> i32 {
    nothing!();
    input
}

pub fn local_struct() {
    make_user!();
    let user: User = User;
    user
}

pub fn local_use() {
    import_id!();
    let id: Id = GlobalId;
    id
}
"#,
        expect![[r#"
            package body_stmt_macro_fixture

            body_stmt_macro_fixture [lib]
            body b0 fn body_stmt_macro_fixture[lib]::crate::steps @ 26:1-29:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1, v2
            bindings
            - v0 param input `input`: i32 => i32 @ 26:14-26:19
            - v1 let doubled `make_steps!(input)` => i32 @ 27:5-27:23
            - v2 let tripled `make_steps!(input)` => i32 @ 27:5-27:23
            body
            expr e7 block s1 => i32 @ 26:33-29:2
              stmt s0 let v1 @ 27:5-27:23
                initializer
                  expr e2 binary + => i32 @ 27:5-27:23
                    lhs
                      expr e0 path input -> local v0 => i32 @ 27:5-27:23
                    rhs
                      expr e1 path input -> local v0 => i32 @ 27:5-27:23
              stmt s1 let v2 @ 27:5-27:23
                initializer
                  expr e5 binary + => i32 @ 27:5-27:23
                    lhs
                      expr e3 path doubled -> local v1 => i32 @ 27:5-27:23
                    rhs
                      expr e4 path input -> local v0 => i32 @ 27:5-27:23
              tail
                expr e6 path tripled -> local v2 => i32 @ 28:5-28:12


            body b1 fn body_stmt_macro_fixture[lib]::crate::empty @ 31:1-34:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 param input `input`: i32 => i32 @ 31:14-31:19
            body
            expr e1 block s1 => i32 @ 31:33-34:2
              tail
                expr e0 path input -> local v0 => i32 @ 33:5-33:10


            body b2 fn body_stmt_macro_fixture[lib]::crate::local_struct @ 36:1-40:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0; source_items i0
            source_items
            - i0 struct User @ 37:5-37:17
            bindings
            - v0 let user `user`: User => nominal struct fn body_stmt_macro_fixture[lib]::crate::local_struct::User @ 37:5-37:17 @ 38:9-38:13
            body
            expr e2 block s1 => nominal struct fn body_stmt_macro_fixture[lib]::crate::local_struct::User @ 37:5-37:17 @ 36:23-40:2
              stmt s0 source_item i0 @ 37:5-37:17
              stmt s1 let v0: User @ 38:5-38:27
                initializer
                  expr e0 path User -> struct fn body_stmt_macro_fixture[lib]::crate::local_struct::User @ 37:5-37:17 => nominal struct fn body_stmt_macro_fixture[lib]::crate::local_struct::User @ 37:5-37:17 @ 38:22-38:26
              tail
                expr e1 path user -> local v0 => nominal struct fn body_stmt_macro_fixture[lib]::crate::local_struct::User @ 37:5-37:17 @ 39:5-39:9


            body b3 fn body_stmt_macro_fixture[lib]::crate::local_use @ 42:1-46:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0; source_items i0
            source_items
            - i0 use <unnamed> @ 43:5-43:17
            bindings
            - v0 let id `id`: Id => nominal struct body_stmt_macro_fixture[lib]::crate::GlobalId @ 44:9-44:11
            body
            expr e2 block s1 => nominal struct body_stmt_macro_fixture[lib]::crate::GlobalId @ 42:20-46:2
              stmt s0 source_item i0 @ 43:5-43:17
              stmt s1 let v0: Id @ 44:5-44:27
                initializer
                  expr e0 path GlobalId -> item struct body_stmt_macro_fixture[lib]::crate::GlobalId => nominal struct body_stmt_macro_fixture[lib]::crate::GlobalId @ 44:18-44:26
              tail
                expr e1 path id -> local v0 => nominal struct body_stmt_macro_fixture[lib]::crate::GlobalId @ 45:5-45:7
        "#]],
    );
}

#[test]
fn macro_pattern_expands_in_let_binding() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_macro_let_pattern_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! bind_pair {
    ($left:ident, $right:ident) => {
        ($left, $right)
    };
}

pub fn use_it(input: (i32, i32)) -> i32 {
    let bind_pair!(left, right) = input;
    left + right
}
"#,
        expect![[r#"
            package body_macro_let_pattern_fixture

            body_macro_let_pattern_fixture [lib]
            body b0 fn body_macro_let_pattern_fixture[lib]::crate::use_it @ 7:1-10:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1, v2
            bindings
            - v0 param input `input`: (i32, i32) => (i32, i32) @ 7:15-7:20
            - v1 let left `bind_pair!(left, right)` => i32 @ 8:9-8:32
            - v2 let right `right` => i32 @ 8:26-8:31
            body
            expr e4 block s1 => i32 @ 7:41-10:2
              stmt s0 let v1, v2 @ 8:5-8:41
                initializer
                  expr e0 path input -> local v0 => (i32, i32) @ 8:35-8:40
              tail
                expr e3 binary + => i32 @ 9:5-9:17
                  lhs
                    expr e1 path left -> local v1 => i32 @ 9:5-9:9
                  rhs
                    expr e2 path right -> local v2 => i32 @ 9:12-9:17
        "#]],
    );
}

#[test]
fn macro_pattern_expands_in_if_let() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_macro_if_let_pattern_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Maybe {
    Some(i32),
    None,
}

macro_rules! some_value {
    ($value:ident) => {
        Maybe::Some($value)
    };
}

pub fn use_it(input: Maybe) -> i32 {
    if let some_value!(value) = input {
        value
    } else {
        0
    }
}
"#,
        expect![[r#"
            package body_macro_if_let_pattern_fixture

            body_macro_if_let_pattern_fixture [lib]
            body b0 fn body_macro_if_let_pattern_fixture[lib]::crate::use_it @ 12:1-18:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            - s2 parent s1: v1
            - s3 parent s2: <none>
            - s4 parent s1: <none>
            bindings
            - v0 param input `input`: Maybe => nominal enum body_macro_if_let_pattern_fixture[lib]::crate::Maybe @ 12:15-12:20
            - v1 let value `value` => i32 @ 13:24-13:29
            body
            expr e7 block s1 => i32 @ 12:36-18:2
              tail
                expr e6 if => i32 @ 13:5-17:6
                  condition
                    expr e1 let s2 v1 => bool @ 13:8-13:38
                      initializer
                        expr e0 path input -> local v0 => nominal enum body_macro_if_let_pattern_fixture[lib]::crate::Maybe @ 13:33-13:38
                  then
                    expr e3 block s3 => i32 @ 13:39-15:6
                      tail
                        expr e2 path value -> local v1 => i32 @ 14:9-14:14
                  else
                    expr e5 block s4 => i32 @ 15:12-17:6
                      tail
                        expr e4 literal int `0` => i32 @ 16:9-16:10
        "#]],
    );
}

#[test]
fn macro_type_expands_in_let_annotation_and_cast() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_macro_type_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

macro_rules! user_ty {
    () => {
        User
    };
}

macro_rules! bool_ty {
    () => {
        bool
    };
}

pub fn use_it(input: User, flag: u8) -> User {
    let user: user_ty!() = input;
    let casted = flag as bool_ty!();
    user
}
"#,
        expect![[r#"
            package body_macro_type_fixture

            body_macro_type_fixture [lib]
            body b0 fn body_macro_type_fixture[lib]::crate::use_it @ 15:1-19:2
            scopes
            - s0 parent <none>: v0, v1
            - s1 parent s0: v2, v3
            bindings
            - v0 param input `input`: User => nominal struct body_macro_type_fixture[lib]::crate::User @ 15:15-15:20
            - v1 param flag `flag`: u8 => u8 @ 15:28-15:32
            - v2 let user `user`: User => nominal struct body_macro_type_fixture[lib]::crate::User @ 16:9-16:13
            - v3 let casted `casted` => bool @ 17:9-17:15
            body
            expr e4 block s1 => nominal struct body_macro_type_fixture[lib]::crate::User @ 15:46-19:2
              stmt s0 let v2: User @ 16:5-16:34
                initializer
                  expr e0 path input -> local v0 => nominal struct body_macro_type_fixture[lib]::crate::User @ 16:28-16:33
              stmt s1 let v3 @ 17:5-17:37
                initializer
                  expr e2 cast as bool => bool @ 17:18-17:36
                    inner
                      expr e1 path flag -> local v1 => u8 @ 17:18-17:22
              tail
                expr e3 path user -> local v2 => nominal struct body_macro_type_fixture[lib]::crate::User @ 18:5-18:9
        "#]],
    );
}

#[test]
fn imported_macro_expands_inside_function_body() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_imported_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod macros {
    macro_rules! make_value {
        ($input:expr) => {
            $input + $input
        };
    }

    pub(crate) use make_value;
}

use macros::make_value;

pub fn use_it(input: i32) -> i32 {
    make_value!(input)
}
"#,
        expect![[r#"
            package body_imported_macro_fixture

            body_imported_macro_fixture [lib]
            body b0 fn body_imported_macro_fixture[lib]::crate::use_it @ 13:1-15:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 param input `input`: i32 => i32 @ 13:15-13:20
            body
            expr e3 block s1 => i32 @ 13:34-15:2
              tail
                expr e2 binary + => i32 @ 14:5-14:23
                  lhs
                    expr e0 path input -> local v0 => i32 @ 14:5-14:23
                  rhs
                    expr e1 path input -> local v0 => i32 @ 14:17-14:22
        "#]],
    );
}

#[test]
fn generated_body_macro_calls_use_dollar_crate_definition_crate() {
    check_project_body_ir_with_sysroot(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub fn dep_value() -> i32 {
    7
}

#[macro_export]
macro_rules! inner_value {
    () => {
        $crate::dep_value()
    };
}

macro_rules! outer_select_value {
    () => {
        cfg_select! {
            _ => { $crate::inner_value!() },
        }
    };
}

macro_rules! outer_value {
    () => {
        $crate::inner_value!()
    };
}

pub use outer_select_value;
pub use outer_value;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
use dep::{outer_select_value, outer_value};

#[macro_export]
macro_rules! inner_value {
    () => {
        false
    };
}

pub fn direct() -> i32 {
    outer_value!()
}

pub fn via_cfg_select() -> i32 {
    outer_select_value!()
}

//- /sysroot/library/core/src/lib.rs
pub struct Core;

//- /sysroot/library/alloc/src/lib.rs
pub struct Alloc;

//- /sysroot/library/std/src/lib.rs
#[rustc_builtin_macro]
#[macro_export]
macro_rules! cfg_select {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

pub mod prelude {
    pub mod rust_2024 {
        pub use crate::cfg_select;
    }
}
"#,
        expect![[r#"
            package alloc

            alloc [lib]
            skipped

            package app

            app [lib]
            body b0 fn app[lib]::crate::direct @ 10:1-12:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e2 block s1 => i32 @ 10:24-12:2
              tail
                expr e1 call => i32 @ 11:5-11:19
                  callee
                    expr e0 path $crate::dep_value -> item fn dep[lib]::crate::dep_value => <unknown> @ 11:5-11:19


            body b1 fn app[lib]::crate::via_cfg_select @ 14:1-16:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e2 block s1 => i32 @ 14:32-16:2
              tail
                expr e1 call => i32 @ 15:5-15:26
                  callee
                    expr e0 path $crate::dep_value -> item fn dep[lib]::crate::dep_value => <unknown> @ 15:5-15:26


            package core

            core [lib]
            skipped

            package dep

            dep [lib]
            body b0 fn dep[lib]::crate::dep_value @ 1:1-3:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e1 block s1 => i32 @ 1:27-3:2
              tail
                expr e0 literal int `7` => i32 @ 2:5-2:6


            package std

            std [lib]
            skipped
        "#]],
    );
}

#[test]
fn cfg_select_builtin_expands_body_syntax_and_respects_shadowing() {
    check_project_body_ir_with_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "body_cfg_select_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn expr(input: i32) -> i32 {
    let selected = cfg_select! {
        false => { unresolved },
        not(test) => { input + 1 },
        _ => { 0 },
    };

    let fallback = cfg_select! {
        false => { unresolved },
        _ => { input },
    };

    selected + fallback
}

pub fn stmt(input: i32) -> i32 {
    cfg_select! {
        false => {
            let broken = ;
        },
        not(test) => {
            let doubled = input + input;
            let tripled = doubled + input;
        },
        _ => {
            let tripled = 0;
        },
    };

    tripled
}

pub mod local {
    macro_rules! cfg_select {
        ($($tt:tt)*) => {
            true
        };
    }

    pub fn shadow() -> bool {
        cfg_select! {
            _ => { 0 },
        }
    }
}

//- /sysroot/library/core/src/lib.rs
pub struct Core;

//- /sysroot/library/alloc/src/lib.rs
pub struct Alloc;

//- /sysroot/library/std/src/lib.rs
#[rustc_builtin_macro]
#[macro_export]
macro_rules! cfg_select {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

pub mod prelude {
    pub mod rust_2024 {
        pub use crate::cfg_select;
    }
}
"#,
        expect![[r#"
            package alloc

            alloc [lib]
            skipped

            package body_cfg_select_fixture

            body_cfg_select_fixture [lib]
            body b0 fn body_cfg_select_fixture[lib]::crate::expr @ 1:1-14:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1, v2
            bindings
            - v0 param input `input`: i32 => i32 @ 1:13-1:18
            - v1 let selected `selected` => i32 @ 2:9-2:17
            - v2 let fallback `fallback` => i32 @ 8:9-8:17
            body
            expr e7 block s1 => i32 @ 1:32-14:2
              stmt s0 let v1 @ 2:5-6:7
                initializer
                  expr e2 binary + => i32 @ 2:20-6:6
                    lhs
                      expr e0 path input -> local v0 => i32 @ 2:20-6:6
                    rhs
                      expr e1 literal int `1` => i32 @ 4:32-4:33
              stmt s1 let v2 @ 8:5-11:7
                initializer
                  expr e3 path input -> local v0 => i32 @ 10:16-10:21
              tail
                expr e6 binary + => i32 @ 13:5-13:24
                  lhs
                    expr e4 path selected -> local v1 => i32 @ 13:5-13:13
                  rhs
                    expr e5 path fallback -> local v2 => i32 @ 13:16-13:24


            body b1 fn body_cfg_select_fixture[lib]::crate::stmt @ 16:1-31:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1, v2
            bindings
            - v0 param input `input`: i32 => i32 @ 16:13-16:18
            - v1 let doubled `cfg_select! { false => { let broken = ; }, not(test) => { let doubled = input + input; let tripled = doubled + input; }, _ => { let tripled = 0; }, }` => i32 @ 17:5-28:6
            - v2 let tripled `cfg_select! { false => { let broken = ; }, not(test) => { let doubled = input + input; let tripled = doubled + input; }, _ => { let tripled = 0; }, }` => i32 @ 17:5-28:6
            body
            expr e7 block s1 => i32 @ 16:32-31:2
              stmt s0 let v1 @ 17:5-28:6
                initializer
                  expr e2 binary + => i32 @ 17:5-28:6
                    lhs
                      expr e0 path input -> local v0 => i32 @ 17:5-28:6
                    rhs
                      expr e1 path input -> local v0 => i32 @ 17:5-28:6
              stmt s1 let v2 @ 17:5-28:6
                initializer
                  expr e5 binary + => i32 @ 17:5-28:6
                    lhs
                      expr e3 path doubled -> local v1 => i32 @ 17:5-28:6
                    rhs
                      expr e4 path input -> local v0 => i32 @ 17:5-28:6
              tail
                expr e6 path tripled -> local v2 => i32 @ 30:5-30:12


            body b2 fn body_cfg_select_fixture[lib]::crate::local::shadow @ 40:5-44:6
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e1 block s1 => bool @ 40:29-44:6
              tail
                expr e0 literal bool `cfg_select! { _ => { 0 }, }` => bool @ 41:9-43:10


            package core

            core [lib]
            skipped

            package std

            std [lib]
            skipped
        "#]],
    );
}

#[test]
fn format_family_builtins_resolve_through_sysroot_and_shadow_normally() {
    check_project_body_ir_with_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "body_format_family_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn direct() {
    let args = format_args!("hello");
    let args_nl = format_args_nl!("hello");
    args_nl
}

pub fn qualified() {
    let args = std::format_args!("hello");
    args
}

pub fn aliased() {
    let direct = format_args!("hello");
    let aliased = my_format_args!("hello");
    aliased
}

pub fn library() {
    let args = format!("hello {}", 1);
    args
}

pub mod shadow {
    macro_rules! format {
        () => {
            92
        };
    }

    macro_rules! format_args {
        () => {
            93
        };
    }

    pub fn local_format() -> i32 {
        let value = format!();
        value
    }

    pub fn local_format_args() -> i32 {
        let value = format_args!();
        value
    }
}

//- /sysroot/library/core/src/lib.rs
pub mod fmt {
    pub struct Arguments;
}

//- /sysroot/library/alloc/src/lib.rs
pub struct Alloc;

//- /sysroot/library/std/src/lib.rs
#[rustc_builtin_macro]
#[macro_export]
macro_rules! format_args {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! format_args_nl {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[macro_export]
macro_rules! format {
    ($($args:tt)*) => {
        $crate::__export::format_args!($($args)*)
    };
}

pub mod __export {
    pub use crate::format_args;
}

pub mod macros {
    pub use crate::format_args as my_format_args;
}

pub mod prelude {
    pub mod rust_2024 {
        pub use crate::format;
        pub use crate::format_args;
        pub use crate::format_args_nl;
        pub use crate::macros::my_format_args;
    }
}
"#,
        expect![[r#"
            package alloc

            alloc [lib]
            skipped

            package body_format_family_fixture

            body_format_family_fixture [lib]
            body b0 fn body_format_family_fixture[lib]::crate::direct @ 1:1-5:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1
            bindings
            - v0 let args `args` => nominal struct core[lib]::crate::fmt::Arguments @ 2:9-2:13
            - v1 let args_nl `args_nl` => nominal struct core[lib]::crate::fmt::Arguments @ 3:9-3:16
            body
            expr e3 block s1 => nominal struct core[lib]::crate::fmt::Arguments @ 1:17-5:2
              stmt s0 let v0 @ 2:5-2:38
                initializer
                  expr e0 builtin_macro format_args => nominal struct core[lib]::crate::fmt::Arguments @ 2:16-2:37
              stmt s1 let v1 @ 3:5-3:44
                initializer
                  expr e1 builtin_macro format_args_nl => nominal struct core[lib]::crate::fmt::Arguments @ 3:19-3:43
              tail
                expr e2 path args_nl -> local v1 => nominal struct core[lib]::crate::fmt::Arguments @ 4:5-4:12


            body b1 fn body_format_family_fixture[lib]::crate::qualified @ 7:1-10:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0
            bindings
            - v0 let args `args` => nominal struct core[lib]::crate::fmt::Arguments @ 8:9-8:13
            body
            expr e2 block s1 => nominal struct core[lib]::crate::fmt::Arguments @ 7:20-10:2
              stmt s0 let v0 @ 8:5-8:43
                initializer
                  expr e0 builtin_macro format_args => nominal struct core[lib]::crate::fmt::Arguments @ 8:16-8:42
              tail
                expr e1 path args -> local v0 => nominal struct core[lib]::crate::fmt::Arguments @ 9:5-9:9


            body b2 fn body_format_family_fixture[lib]::crate::aliased @ 12:1-16:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1
            bindings
            - v0 let direct `direct` => nominal struct core[lib]::crate::fmt::Arguments @ 13:9-13:15
            - v1 let aliased `aliased` => nominal struct core[lib]::crate::fmt::Arguments @ 14:9-14:16
            body
            expr e3 block s1 => nominal struct core[lib]::crate::fmt::Arguments @ 12:18-16:2
              stmt s0 let v0 @ 13:5-13:40
                initializer
                  expr e0 builtin_macro format_args => nominal struct core[lib]::crate::fmt::Arguments @ 13:18-13:39
              stmt s1 let v1 @ 14:5-14:44
                initializer
                  expr e1 builtin_macro format_args => nominal struct core[lib]::crate::fmt::Arguments @ 14:19-14:43
              tail
                expr e2 path aliased -> local v1 => nominal struct core[lib]::crate::fmt::Arguments @ 15:5-15:12


            body b3 fn body_format_family_fixture[lib]::crate::library @ 18:1-21:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0
            bindings
            - v0 let args `args` => nominal struct core[lib]::crate::fmt::Arguments @ 19:9-19:13
            body
            expr e2 block s1 => nominal struct core[lib]::crate::fmt::Arguments @ 18:18-21:2
              stmt s0 let v0 @ 19:5-19:39
                initializer
                  expr e0 builtin_macro format_args => nominal struct core[lib]::crate::fmt::Arguments @ 19:16-19:38
              tail
                expr e1 path args -> local v0 => nominal struct core[lib]::crate::fmt::Arguments @ 20:5-20:9


            body b4 fn body_format_family_fixture[lib]::crate::shadow::local_format @ 36:5-39:6
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0
            bindings
            - v0 let value `value` => i32 @ 37:13-37:18
            body
            expr e2 block s1 => i32 @ 36:34-39:6
              stmt s0 let v0 @ 37:9-37:31
                initializer
                  expr e0 literal int `format!()` => i32 @ 37:21-37:30
              tail
                expr e1 path value -> local v0 => i32 @ 38:9-38:14


            body b5 fn body_format_family_fixture[lib]::crate::shadow::local_format_args @ 41:5-44:6
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0
            bindings
            - v0 let value `value` => i32 @ 42:13-42:18
            body
            expr e2 block s1 => i32 @ 41:39-44:6
              stmt s0 let v0 @ 42:9-42:36
                initializer
                  expr e0 literal int `format_args!()` => i32 @ 42:21-42:35
              tail
                expr e1 path value -> local v0 => i32 @ 43:9-43:14


            package core

            core [lib]
            skipped

            package std

            std [lib]
            skipped
        "#]],
    );
}

#[test]
fn ambiguous_prelude_macro_blocks_body_builtin_resolution() {
    check_project_body_ir_with_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "body_ambiguous_prelude_builtin_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    let value = format_args!();
    value
}

//- /sysroot/library/core/src/lib.rs
pub mod fmt {
    pub struct Arguments;
}

//- /sysroot/library/alloc/src/lib.rs
pub struct Alloc;

//- /sysroot/library/std/src/lib.rs
pub mod first {
    macro_rules! format_args {
        () => {
            1u8
        };
    }

    pub use format_args;
}

pub mod second {
    macro_rules! format_args {
        () => {
            2u8
        };
    }

    pub use format_args;
}

pub mod prelude {
    pub mod rust_2024 {
        pub use crate::first::format_args;
        pub use crate::second::format_args;
    }
}
"#,
        expect![[r#"
            package alloc

            alloc [lib]
            skipped

            package body_ambiguous_prelude_builtin_fixture

            body_ambiguous_prelude_builtin_fixture [lib]
            body b0 fn body_ambiguous_prelude_builtin_fixture[lib]::crate::use_it @ 1:1-4:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0
            bindings
            - v0 let value `value` => <unknown> @ 2:9-2:14
            body
            expr e2 block s1 => <unknown> @ 1:17-4:2
              stmt s0 let v0 @ 2:5-2:32
                initializer
                  expr e0 unknown `format_args!()` => <unknown> @ 2:17-2:31
              tail
                expr e1 path value -> local v0 => <unknown> @ 3:5-3:10


            package core

            core [lib]
            skipped

            package std

            std [lib]
            skipped
        "#]],
    );
}

#[test]
fn common_builtin_macros_lower_to_body_expression_types() {
    check_project_body_ir_with_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "body_builtin_macro_types_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    let cfg_value = cfg!(target_os = "linux");
    let stringified = stringify!(a + b);
    let concatenated = concat!("a", "b");
    let env_value = env!("HOME");
    let maybe_env = option_env!("HOME");
    let included = include_str!("missing.txt");
    let bytes = include_bytes!("missing.bin");
    let file_name = file!();
    let module_name = module_path!();
    let line_no = line!();
    let column_no = column!();
    maybe_env
}

//- /sysroot/library/core/src/lib.rs
pub mod option {
    pub enum Option<T> {
        Some(T),
        None,
    }
}

//- /sysroot/library/alloc/src/lib.rs
pub struct Alloc;

//- /sysroot/library/std/src/lib.rs
#[rustc_builtin_macro]
#[macro_export]
macro_rules! cfg {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! column {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! concat {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! env {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! file {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! include_bytes {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! include_str {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! line {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! module_path {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! option_env {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! stringify {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

pub mod prelude {
    pub mod rust_2024 {
        pub use crate::cfg;
        pub use crate::column;
        pub use crate::concat;
        pub use crate::env;
        pub use crate::file;
        pub use crate::include_bytes;
        pub use crate::include_str;
        pub use crate::line;
        pub use crate::module_path;
        pub use crate::option_env;
        pub use crate::stringify;
    }
}
"#,
        expect![[r#"
            package alloc

            alloc [lib]
            skipped

            package body_builtin_macro_types_fixture

            body_builtin_macro_types_fixture [lib]
            body b0 fn body_builtin_macro_types_fixture[lib]::crate::use_it @ 1:1-14:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10
            bindings
            - v0 let cfg_value `cfg_value` => bool @ 2:9-2:18
            - v1 let stringified `stringified` => &str @ 3:9-3:20
            - v2 let concatenated `concatenated` => &str @ 4:9-4:21
            - v3 let env_value `env_value` => &str @ 5:9-5:18
            - v4 let maybe_env `maybe_env` => nominal enum core[lib]::crate::option::Option<&str> @ 6:9-6:18
            - v5 let included `included` => &str @ 7:9-7:17
            - v6 let bytes `bytes` => &[u8] @ 8:9-8:14
            - v7 let file_name `file_name` => &str @ 9:9-9:18
            - v8 let module_name `module_name` => &str @ 10:9-10:20
            - v9 let line_no `line_no` => u32 @ 11:9-11:16
            - v10 let column_no `column_no` => u32 @ 12:9-12:18
            body
            expr e12 block s1 => nominal enum core[lib]::crate::option::Option<&str> @ 1:17-14:2
              stmt s0 let v0 @ 2:5-2:47
                initializer
                  expr e0 builtin_macro cfg => bool @ 2:21-2:46
              stmt s1 let v1 @ 3:5-3:41
                initializer
                  expr e1 builtin_macro stringify => &str @ 3:23-3:40
              stmt s2 let v2 @ 4:5-4:42
                initializer
                  expr e2 builtin_macro concat => &str @ 4:24-4:41
              stmt s3 let v3 @ 5:5-5:34
                initializer
                  expr e3 builtin_macro env => &str @ 5:21-5:33
              stmt s4 let v4 @ 6:5-6:41
                initializer
                  expr e4 builtin_macro option_env => nominal enum core[lib]::crate::option::Option<&str> @ 6:21-6:40
              stmt s5 let v5 @ 7:5-7:48
                initializer
                  expr e5 builtin_macro include_str => &str @ 7:20-7:47
              stmt s6 let v6 @ 8:5-8:47
                initializer
                  expr e6 builtin_macro include_bytes => &[u8] @ 8:17-8:46
              stmt s7 let v7 @ 9:5-9:29
                initializer
                  expr e7 builtin_macro file => &str @ 9:21-9:28
              stmt s8 let v8 @ 10:5-10:38
                initializer
                  expr e8 builtin_macro module_path => &str @ 10:23-10:37
              stmt s9 let v9 @ 11:5-11:27
                initializer
                  expr e9 builtin_macro line => u32 @ 11:19-11:26
              stmt s10 let v10 @ 12:5-12:31
                initializer
                  expr e10 builtin_macro column => u32 @ 12:21-12:30
              tail
                expr e11 path maybe_env -> local v4 => nominal enum core[lib]::crate::option::Option<&str> @ 13:5-13:14


            package core

            core [lib]
            skipped

            package std

            std [lib]
            skipped
        "#]],
    );
}
