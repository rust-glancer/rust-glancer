use expect_test::expect;

use super::utils::check_project_body_ir;

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
fn expands_module_visible_macro_statement_bodies() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_stmt_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_steps {
    ($input:expr) => {
        let doubled = $input + $input;
        let tripled = doubled + $input;
    };
}

pub fn use_it(input: i32) -> i32 {
    make_steps!(input);
    tripled
}
"#,
        expect![[r#"
            package body_stmt_macro_fixture

            body_stmt_macro_fixture [lib]
            body b0 fn body_stmt_macro_fixture[lib]::crate::use_it @ 8:1-11:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1, v2
            bindings
            - v0 param input `input`: i32 => i32 @ 8:15-8:20
            - v1 let doubled `make_steps!(input)` => i32 @ 9:5-9:23
            - v2 let tripled `make_steps!(input)` => i32 @ 9:5-9:23
            body
            expr e7 block s1 => i32 @ 8:34-11:2
              stmt s0 let v1 @ 9:5-9:23
                initializer
                  expr e2 binary + => i32 @ 9:5-9:23
                    lhs
                      expr e0 path input -> local v0 => i32 @ 9:5-9:23
                    rhs
                      expr e1 path input -> local v0 => i32 @ 9:5-9:23
              stmt s1 let v2 @ 9:5-9:23
                initializer
                  expr e5 binary + => i32 @ 9:5-9:23
                    lhs
                      expr e3 path doubled -> local v1 => i32 @ 9:5-9:23
                    rhs
                      expr e4 path input -> local v0 => i32 @ 9:5-9:23
              tail
                expr e6 path tripled -> local v2 => i32 @ 10:5-10:12
        "#]],
    );
}

#[test]
fn skips_empty_macro_statement_expansion() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_stmt_empty_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! nothing {
    () => {};
}

pub fn use_it(input: i32) -> i32 {
    nothing!();
    input
}
"#,
        expect![[r#"
            package body_stmt_empty_macro_fixture

            body_stmt_empty_macro_fixture [lib]
            body b0 fn body_stmt_empty_macro_fixture[lib]::crate::use_it @ 5:1-8:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 param input `input`: i32 => i32 @ 5:15-5:20
            body
            expr e1 block s1 => i32 @ 5:34-8:2
              tail
                expr e0 path input -> local v0 => i32 @ 7:5-7:10
        "#]],
    );
}

#[test]
fn macro_statement_expands_to_body_local_struct() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_macro_local_struct_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_user {
    () => {
        struct User;
    };
}

pub fn use_it() {
    make_user!();
    let user: User = User;
    user
}
"#,
        expect![[r#"
            package body_macro_local_struct_fixture

            body_macro_local_struct_fixture [lib]
            body b0 fn body_macro_local_struct_fixture[lib]::crate::use_it @ 7:1-11:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0; source_items i0
            source_items
            - i0 struct User @ 8:5-8:17
            bindings
            - v0 let user `user`: User => nominal struct fn body_macro_local_struct_fixture[lib]::crate::use_it::User @ 8:5-8:17 @ 9:9-9:13
            body
            expr e2 block s1 => nominal struct fn body_macro_local_struct_fixture[lib]::crate::use_it::User @ 8:5-8:17 @ 7:17-11:2
              stmt s0 source_item i0 @ 8:5-8:17
              stmt s1 let v0: User @ 9:5-9:27
                initializer
                  expr e0 path User -> struct fn body_macro_local_struct_fixture[lib]::crate::use_it::User @ 8:5-8:17 => nominal struct fn body_macro_local_struct_fixture[lib]::crate::use_it::User @ 8:5-8:17 @ 9:22-9:26
              tail
                expr e1 path user -> local v0 => nominal struct fn body_macro_local_struct_fixture[lib]::crate::use_it::User @ 8:5-8:17 @ 10:5-10:9
        "#]],
    );
}

#[test]
fn macro_statement_expands_to_body_local_use() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_macro_local_use_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

macro_rules! import_id {
    () => {
        use crate::GlobalId as Id;
    };
}

pub fn use_it() {
    import_id!();
    let id: Id = GlobalId;
    id
}
"#,
        expect![[r#"
            package body_macro_local_use_fixture

            body_macro_local_use_fixture [lib]
            body b0 fn body_macro_local_use_fixture[lib]::crate::use_it @ 9:1-13:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0; source_items i0
            source_items
            - i0 use <unnamed> @ 10:5-10:17
            bindings
            - v0 let id `id`: Id => nominal struct body_macro_local_use_fixture[lib]::crate::GlobalId @ 11:9-11:11
            body
            expr e2 block s1 => nominal struct body_macro_local_use_fixture[lib]::crate::GlobalId @ 9:17-13:2
              stmt s0 source_item i0 @ 10:5-10:17
              stmt s1 let v0: Id @ 11:5-11:27
                initializer
                  expr e0 path GlobalId -> item struct body_macro_local_use_fixture[lib]::crate::GlobalId => nominal struct body_macro_local_use_fixture[lib]::crate::GlobalId @ 11:18-11:26
              tail
                expr e1 path id -> local v0 => nominal struct body_macro_local_use_fixture[lib]::crate::GlobalId @ 12:5-12:7
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
fn dependency_macro_dollar_crate_resolves_to_definition_crate_in_body() {
    check_project_body_ir(
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

macro_rules! make_dep_value {
    () => {
        $crate::dep_value()
    };
}

pub use make_dep_value;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
use dep::make_dep_value;

pub fn use_it() -> i32 {
    make_dep_value!()
}
"#,
        expect![[r#"
            package app

            app [lib]
            body b0 fn app[lib]::crate::use_it @ 3:1-5:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e2 block s1 => i32 @ 3:24-5:2
              tail
                expr e1 call => i32 @ 4:5-4:22
                  callee
                    expr e0 path $crate::dep_value -> item fn dep[lib]::crate::dep_value => <unknown> @ 4:5-4:22


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
        "#]],
    );
}

#[test]
fn cfg_select_builtin_expands_expression_body() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_cfg_select_expr_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it(input: i32) -> i32 {
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
"#,
        expect![[r#"
            package body_cfg_select_expr_fixture

            body_cfg_select_expr_fixture [lib]
            body b0 fn body_cfg_select_expr_fixture[lib]::crate::use_it @ 1:1-14:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1, v2
            bindings
            - v0 param input `input`: i32 => i32 @ 1:15-1:20
            - v1 let selected `selected` => i32 @ 2:9-2:17
            - v2 let fallback `fallback` => i32 @ 8:9-8:17
            body
            expr e7 block s1 => i32 @ 1:34-14:2
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
        "#]],
    );
}

#[test]
fn cfg_select_builtin_expands_statement_body() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_cfg_select_stmt_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it(input: i32) -> i32 {
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
"#,
        expect![[r#"
            package body_cfg_select_stmt_fixture

            body_cfg_select_stmt_fixture [lib]
            body b0 fn body_cfg_select_stmt_fixture[lib]::crate::use_it @ 1:1-16:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1, v2
            bindings
            - v0 param input `input`: i32 => i32 @ 1:15-1:20
            - v1 let doubled `cfg_select! { false => { let broken = ; }, not(test) => { let doubled = input + input; let tripled = doubled + input; }, _ => { let tripled = 0; }, }` => i32 @ 2:5-13:6
            - v2 let tripled `cfg_select! { false => { let broken = ; }, not(test) => { let doubled = input + input; let tripled = doubled + input; }, _ => { let tripled = 0; }, }` => i32 @ 2:5-13:6
            body
            expr e7 block s1 => i32 @ 1:34-16:2
              stmt s0 let v1 @ 2:5-13:6
                initializer
                  expr e2 binary + => i32 @ 2:5-13:6
                    lhs
                      expr e0 path input -> local v0 => i32 @ 2:5-13:6
                    rhs
                      expr e1 path input -> local v0 => i32 @ 2:5-13:6
              stmt s1 let v2 @ 2:5-13:6
                initializer
                  expr e5 binary + => i32 @ 2:5-13:6
                    lhs
                      expr e3 path doubled -> local v1 => i32 @ 2:5-13:6
                    rhs
                      expr e4 path input -> local v0 => i32 @ 2:5-13:6
              tail
                expr e6 path tripled -> local v2 => i32 @ 15:5-15:12
        "#]],
    );
}

#[test]
fn local_cfg_select_macro_shadows_body_builtin() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_cfg_select_shadow_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! cfg_select {
    ($($tt:tt)*) => {
        true
    };
}

pub fn use_it() -> bool {
    cfg_select! {
        _ => { 0 },
    }
}
"#,
        expect![[r#"
            package body_cfg_select_shadow_fixture

            body_cfg_select_shadow_fixture [lib]
            body b0 fn body_cfg_select_shadow_fixture[lib]::crate::use_it @ 7:1-11:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e1 block s1 => bool @ 7:25-11:2
              tail
                expr e0 literal bool `cfg_select! { _ => { 0 }, }` => bool @ 8:5-10:6
        "#]],
    );
}

#[test]
fn format_args_builtin_lowers_as_body_expression() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_format_args_builtin_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod core {
    pub mod fmt {
        pub struct Arguments;
    }
}

pub fn use_it() {
    let args = format_args!("hello");
    let args_nl = format_args_nl!("hello");
    args_nl
}
"#,
        expect![[r#"
            package body_format_args_builtin_fixture

            body_format_args_builtin_fixture [lib]
            body b0 fn body_format_args_builtin_fixture[lib]::crate::use_it @ 7:1-11:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1
            bindings
            - v0 let args `args` => nominal struct body_format_args_builtin_fixture[lib]::crate::core::fmt::Arguments @ 8:9-8:13
            - v1 let args_nl `args_nl` => nominal struct body_format_args_builtin_fixture[lib]::crate::core::fmt::Arguments @ 9:9-9:16
            body
            expr e3 block s1 => nominal struct body_format_args_builtin_fixture[lib]::crate::core::fmt::Arguments @ 7:17-11:2
              stmt s0 let v0 @ 8:5-8:38
                initializer
                  expr e0 builtin_macro format_args => nominal struct body_format_args_builtin_fixture[lib]::crate::core::fmt::Arguments @ 8:16-8:37
              stmt s1 let v1 @ 9:5-9:44
                initializer
                  expr e1 builtin_macro format_args_nl => nominal struct body_format_args_builtin_fixture[lib]::crate::core::fmt::Arguments @ 9:19-9:43
              tail
                expr e2 path args_nl -> local v1 => nominal struct body_format_args_builtin_fixture[lib]::crate::core::fmt::Arguments @ 10:5-10:12
        "#]],
    );
}

#[test]
fn qualified_format_args_builtin_lowers_as_body_expression() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_qualified_format_args_builtin_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod core {
    pub mod fmt {
        pub struct Arguments;
    }
}

pub fn use_it() {
    let args = core::format_args!("hello");
    args
}
"#,
        expect![[r#"
            package body_qualified_format_args_builtin_fixture

            body_qualified_format_args_builtin_fixture [lib]
            body b0 fn body_qualified_format_args_builtin_fixture[lib]::crate::use_it @ 7:1-10:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0
            bindings
            - v0 let args `args` => nominal struct body_qualified_format_args_builtin_fixture[lib]::crate::core::fmt::Arguments @ 8:9-8:13
            body
            expr e2 block s1 => nominal struct body_qualified_format_args_builtin_fixture[lib]::crate::core::fmt::Arguments @ 7:17-10:2
              stmt s0 let v0 @ 8:5-8:44
                initializer
                  expr e0 builtin_macro format_args => nominal struct body_qualified_format_args_builtin_fixture[lib]::crate::core::fmt::Arguments @ 8:16-8:43
              tail
                expr e1 path args -> local v0 => nominal struct body_qualified_format_args_builtin_fixture[lib]::crate::core::fmt::Arguments @ 9:5-9:9
        "#]],
    );
}

#[test]
fn local_format_args_macro_shadows_builtin_in_body() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_format_args_shadow_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! format_args {
    () => {
        92
    };
}

pub fn use_it() -> i32 {
    let value = format_args!();
    value
}
"#,
        expect![[r#"
            package body_format_args_shadow_fixture

            body_format_args_shadow_fixture [lib]
            body b0 fn body_format_args_shadow_fixture[lib]::crate::use_it @ 7:1-10:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0
            bindings
            - v0 let value `value` => i32 @ 8:9-8:14
            body
            expr e2 block s1 => i32 @ 7:24-10:2
              stmt s0 let v0 @ 8:5-8:32
                initializer
                  expr e0 literal int `format_args!()` => i32 @ 8:17-8:31
              tail
                expr e1 path value -> local v0 => i32 @ 9:5-9:10
        "#]],
    );
}

#[test]
fn common_builtin_macros_lower_to_body_expression_types() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_builtin_macro_types_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod core {
    pub mod option {
        pub enum Option<T> {
            Some(T),
            None,
        }
    }
}

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
"#,
        expect![[r#"
            package body_builtin_macro_types_fixture

            body_builtin_macro_types_fixture [lib]
            body b0 fn body_builtin_macro_types_fixture[lib]::crate::use_it @ 10:1-23:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10
            bindings
            - v0 let cfg_value `cfg_value` => bool @ 11:9-11:18
            - v1 let stringified `stringified` => &str @ 12:9-12:20
            - v2 let concatenated `concatenated` => &str @ 13:9-13:21
            - v3 let env_value `env_value` => &str @ 14:9-14:18
            - v4 let maybe_env `maybe_env` => nominal enum body_builtin_macro_types_fixture[lib]::crate::core::option::Option<&str> @ 15:9-15:18
            - v5 let included `included` => &str @ 16:9-16:17
            - v6 let bytes `bytes` => &[u8] @ 17:9-17:14
            - v7 let file_name `file_name` => &str @ 18:9-18:18
            - v8 let module_name `module_name` => &str @ 19:9-19:20
            - v9 let line_no `line_no` => u32 @ 20:9-20:16
            - v10 let column_no `column_no` => u32 @ 21:9-21:18
            body
            expr e12 block s1 => nominal enum body_builtin_macro_types_fixture[lib]::crate::core::option::Option<&str> @ 10:17-23:2
              stmt s0 let v0 @ 11:5-11:47
                initializer
                  expr e0 builtin_macro cfg => bool @ 11:21-11:46
              stmt s1 let v1 @ 12:5-12:41
                initializer
                  expr e1 builtin_macro stringify => &str @ 12:23-12:40
              stmt s2 let v2 @ 13:5-13:42
                initializer
                  expr e2 builtin_macro concat => &str @ 13:24-13:41
              stmt s3 let v3 @ 14:5-14:34
                initializer
                  expr e3 builtin_macro env => &str @ 14:21-14:33
              stmt s4 let v4 @ 15:5-15:41
                initializer
                  expr e4 builtin_macro option_env => nominal enum body_builtin_macro_types_fixture[lib]::crate::core::option::Option<&str> @ 15:21-15:40
              stmt s5 let v5 @ 16:5-16:48
                initializer
                  expr e5 builtin_macro include_str => &str @ 16:20-16:47
              stmt s6 let v6 @ 17:5-17:47
                initializer
                  expr e6 builtin_macro include_bytes => &[u8] @ 17:17-17:46
              stmt s7 let v7 @ 18:5-18:29
                initializer
                  expr e7 builtin_macro file => &str @ 18:21-18:28
              stmt s8 let v8 @ 19:5-19:38
                initializer
                  expr e8 builtin_macro module_path => &str @ 19:23-19:37
              stmt s9 let v9 @ 20:5-20:27
                initializer
                  expr e9 builtin_macro line => u32 @ 20:19-20:26
              stmt s10 let v10 @ 21:5-21:31
                initializer
                  expr e10 builtin_macro column => u32 @ 21:21-21:30
              tail
                expr e11 path maybe_env -> local v4 => nominal enum body_builtin_macro_types_fixture[lib]::crate::core::option::Option<&str> @ 22:5-22:14
        "#]],
    );
}
