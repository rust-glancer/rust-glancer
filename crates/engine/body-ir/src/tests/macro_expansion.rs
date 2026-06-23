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
                      expr e1 path input -> local v0 => i32 @ 9:21-9:39
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
