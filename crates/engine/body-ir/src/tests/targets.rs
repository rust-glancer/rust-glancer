use expect_test::expect;

use super::utils::check_project_body_ir;

#[test]
fn resolves_body_paths_and_types_inside_bin_root() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_bin_fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[[bin]]
name = "body-bin-fixture"
path = "src/main.rs"

//- /src/lib.rs
pub struct Api;

pub fn make() -> Api {
    Api
}

//- /src/main.rs
fn main() {
    let api: body_bin_fixture::Api = body_bin_fixture::make();
    let again: body_bin_fixture::Api = api;
}
"#,
        expect![[r#"
            package body_bin_fixture

            body_bin_fixture [lib]
            body b0 fn body_bin_fixture[lib]::crate::make @ 3:1-5:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e1 block s1 => nominal struct body_bin_fixture[lib]::crate::Api @ 3:22-5:2
              tail
                expr e0 path Api -> item struct body_bin_fixture[lib]::crate::Api => nominal struct body_bin_fixture[lib]::crate::Api @ 4:5-4:8


            body-bin-fixture [bin]
            body b0 fn body_bin_fixture[bin]::crate::main @ 1:1-4:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1
            bindings
            - v0 let api `api`: body_bin_fixture::Api => nominal struct body_bin_fixture[lib]::crate::Api @ 2:9-2:12
            - v1 let again `again`: body_bin_fixture::Api => nominal struct body_bin_fixture[lib]::crate::Api @ 3:9-3:14
            body
            expr e3 block s1 => () @ 1:11-4:2
              stmt s0 let v0: body_bin_fixture::Api @ 2:5-2:63
                initializer
                  expr e1 call => nominal struct body_bin_fixture[lib]::crate::Api @ 2:38-2:62
                    callee
                      expr e0 path body_bin_fixture::make -> item fn body_bin_fixture[lib]::crate::make => <unknown> @ 2:38-2:60
              stmt s1 let v1: body_bin_fixture::Api @ 3:5-3:44
                initializer
                  expr e2 path api -> local v0 => nominal struct body_bin_fixture[lib]::crate::Api @ 3:40-3:43
        "#]],
    );
}
