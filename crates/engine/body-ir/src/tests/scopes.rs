use expect_test::expect;

use super::utils::check_project_body_ir;

#[test]
fn lowers_scopes_and_resolves_local_bindings() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_scope_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct UserId(u64);

pub fn choose(id: UserId) -> UserId {
    let copied: UserId = id;
    let shadow: UserId = {
        let id: UserId = copied;
        id
    };
    shadow
}
"#,
        expect![[r#"
            package body_scope_fixture

            body_scope_fixture [lib]
            body b0 fn body_scope_fixture[lib]::crate::choose @ 3:1-10:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1, v3
            - s2 parent s1: v2
            bindings
            - v0 param id `id`: UserId => nominal struct body_scope_fixture[lib]::crate::UserId @ 3:15-3:17
            - v1 let copied `copied`: UserId => nominal struct body_scope_fixture[lib]::crate::UserId @ 4:9-4:15
            - v2 let id `id`: UserId => nominal struct body_scope_fixture[lib]::crate::UserId @ 6:13-6:15
            - v3 let shadow `shadow`: UserId => nominal struct body_scope_fixture[lib]::crate::UserId @ 5:9-5:15
            body
            expr e5 block s1 => nominal struct body_scope_fixture[lib]::crate::UserId @ 3:37-10:2
              stmt s0 let v1: UserId @ 4:5-4:29
                initializer
                  expr e0 path id -> local v0 => nominal struct body_scope_fixture[lib]::crate::UserId @ 4:26-4:28
              stmt s2 let v3: UserId @ 5:5-8:7
                initializer
                  expr e3 block s2 => nominal struct body_scope_fixture[lib]::crate::UserId @ 5:26-8:6
                    stmt s1 let v2: UserId @ 6:9-6:33
                      initializer
                        expr e1 path copied -> local v1 => nominal struct body_scope_fixture[lib]::crate::UserId @ 6:26-6:32
                    tail
                      expr e2 path id -> local v2 => nominal struct body_scope_fixture[lib]::crate::UserId @ 7:9-7:11
              tail
                expr e4 path shadow -> local v3 => nominal struct body_scope_fixture[lib]::crate::UserId @ 9:5-9:11
        "#]],
    );
}
