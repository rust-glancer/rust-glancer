use expect_test::expect;

use super::utils::check_project_body_ir;

#[test]
fn propagates_basic_generic_arguments_through_body_types() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_generic_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub struct Slot<T> {
    pub value: T,
}

pub struct Wrapper<T> {
    pub slot: Slot<T>,
}

pub fn use_it() {
    let wrapper: Wrapper<User>;
    let user = wrapper.slot.value;
}
"#,
        expect![[r#"
            package body_generic_fixture

            body_generic_fixture [lib]
            body b0 fn body_generic_fixture[lib]::crate::use_it @ 11:1-14:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1
            bindings
            - v0 let wrapper `wrapper`: Wrapper<User> => nominal struct body_generic_fixture[lib]::crate::Wrapper<nominal struct body_generic_fixture[lib]::crate::User> @ 12:9-12:16
            - v1 let user `user` => nominal struct body_generic_fixture[lib]::crate::User @ 13:9-13:13
            body
            expr e3 block s1 => () @ 11:17-14:2
              stmt s0 let v0: Wrapper<User> @ 12:5-12:32
              stmt s1 let v1 @ 13:5-13:35
                initializer
                  expr e2 field value -> field struct body_generic_fixture[lib]::crate::Slot::value => nominal struct body_generic_fixture[lib]::crate::User @ 13:16-13:34
                    base
                      expr e1 field slot -> field struct body_generic_fixture[lib]::crate::Wrapper::slot => nominal struct body_generic_fixture[lib]::crate::Slot<nominal struct body_generic_fixture[lib]::crate::User> @ 13:16-13:28
                        base
                          expr e0 path wrapper -> local v0 => nominal struct body_generic_fixture[lib]::crate::Wrapper<nominal struct body_generic_fixture[lib]::crate::User> @ 13:16-13:23
        "#]],
    );
}
