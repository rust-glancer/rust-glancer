use expect_test::expect;

use super::utils::check_project_body_ir;

#[test]
fn resolves_body_local_structs_before_module_structs() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_local_item_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn use_it() {
    struct User;
    let local: User = User;
    {
        struct User;
        let nested: User = User;
    }
    let again: User = User;
}
"#,
        expect![[r#"
            package body_local_item_fixture

            body_local_item_fixture [lib]
            body b0 fn body_local_item_fixture[lib]::crate::use_it @ 3:1-11:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v2; items i0
            - s2 parent s1: v1; items i1
            items
            - i0 struct User @ 4:5-4:17
            - i1 struct User @ 7:9-7:21
            bindings
            - v0 let local `local`: User => local nominal struct fn body_local_item_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 5:9-5:14
            - v1 let nested `nested`: User => local nominal struct fn body_local_item_fixture[lib]::crate::use_it::User @ 7:9-7:21 @ 8:13-8:19
            - v2 let again `again`: User => local nominal struct fn body_local_item_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 10:9-10:14
            body
            expr e4 block s1 => () @ 3:17-11:2
              stmt s0 item i0 @ 4:5-4:17
              stmt s1 let v0: User @ 5:5-5:28
                initializer
                  expr e0 path User -> local item struct fn body_local_item_fixture[lib]::crate::use_it::User @ 4:5-4:17 => local nominal struct fn body_local_item_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 5:23-5:27
              stmt s4 expr @ 6:5-9:6
                expr e2 block s2 => () @ 6:5-9:6
                  stmt s2 item i1 @ 7:9-7:21
                  stmt s3 let v1: User @ 8:9-8:33
                    initializer
                      expr e1 path User -> local item struct fn body_local_item_fixture[lib]::crate::use_it::User @ 7:9-7:21 => local nominal struct fn body_local_item_fixture[lib]::crate::use_it::User @ 7:9-7:21 @ 8:28-8:32
              stmt s5 let v2: User @ 10:5-10:28
                initializer
                  expr e3 path User -> local item struct fn body_local_item_fixture[lib]::crate::use_it::User @ 4:5-4:17 => local nominal struct fn body_local_item_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 10:23-10:27
        "#]],
    );
}

#[test]
fn resolves_body_local_struct_fields() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_local_field_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User {
        id: GlobalId,
        pair: Pair,
    }
    struct Pair(GlobalId, GlobalId);

    let user: User;
    let id = user.id;
    let right = user.pair.1;
}
"#,
        expect![[r#"
            package body_local_field_fixture

            body_local_field_fixture [lib]
            body b0 fn body_local_field_fixture[lib]::crate::use_it @ 3:1-13:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1, v2; items i0, i1
            items
            - i0 struct User @ 4:5-7:6
            - i1 struct Pair @ 8:5-8:37
            bindings
            - v0 let user `user`: User => local nominal struct fn body_local_field_fixture[lib]::crate::use_it::User @ 4:5-7:6 @ 10:9-10:13
            - v1 let id `id` => nominal struct body_local_field_fixture[lib]::crate::GlobalId @ 11:9-11:11
            - v2 let right `right` => nominal struct body_local_field_fixture[lib]::crate::GlobalId @ 12:9-12:14
            body
            expr e5 block s1 => () @ 3:17-13:2
              stmt s0 item i0 @ 4:5-7:6
              stmt s1 item i1 @ 8:5-8:37
              stmt s2 let v0: User @ 10:5-10:20
              stmt s3 let v1 @ 11:5-11:22
                initializer
                  expr e1 field id -> field struct fn body_local_field_fixture[lib]::crate::use_it::User @ 4:5-7:6::id => nominal struct body_local_field_fixture[lib]::crate::GlobalId @ 11:14-11:21
                    base
                      expr e0 path user -> local v0 => local nominal struct fn body_local_field_fixture[lib]::crate::use_it::User @ 4:5-7:6 @ 11:14-11:18
              stmt s4 let v2 @ 12:5-12:29
                initializer
                  expr e4 field 1 -> field struct fn body_local_field_fixture[lib]::crate::use_it::Pair @ 8:5-8:37::#1 => nominal struct body_local_field_fixture[lib]::crate::GlobalId @ 12:17-12:28
                    base
                      expr e3 field pair -> field struct fn body_local_field_fixture[lib]::crate::use_it::User @ 4:5-7:6::pair => local nominal struct fn body_local_field_fixture[lib]::crate::use_it::Pair @ 8:5-8:37 @ 12:17-12:26
                        base
                          expr e2 path user -> local v0 => local nominal struct fn body_local_field_fixture[lib]::crate::use_it::User @ 4:5-7:6 @ 12:17-12:21
        "#]],
    );
}

#[test]
fn resolves_body_local_impl_methods() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_local_impl_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User;

    impl User {
        fn id(&self) -> GlobalId {
            missing()
        }

        fn again(&self) -> Self {
            missing()
        }

        fn associated() -> GlobalId {
            missing()
        }
    }

    let user: User;
    let id = user.id();
    let again = user.again();
}
"#,
        expect![[r#"
            package body_local_impl_fixture

            body_local_impl_fixture [lib]
            body b0 fn body_local_impl_fixture[lib]::crate::use_it @ 3:1-23:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1, v2; items i0; impls m0
            items
            - i0 struct User @ 4:5-4:17
            impls
            - m0 impl User => struct fn body_local_impl_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 6:5-18:6
              - f0 fn id(&self) -> GlobalId
              - f1 fn again(&self) -> Self
              - f2 fn associated() -> GlobalId
            bindings
            - v0 let user `user`: User => local nominal struct fn body_local_impl_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 20:9-20:13
            - v1 let id `id` => nominal struct body_local_impl_fixture[lib]::crate::GlobalId @ 21:9-21:11
            - v2 let again `again` => local nominal struct fn body_local_impl_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 22:9-22:14
            body
            expr e4 block s1 => () @ 3:17-23:2
              stmt s0 item i0 @ 4:5-4:17
              stmt s1 impl m0 @ 6:5-18:6
              stmt s2 let v0: User @ 20:5-20:20
              stmt s3 let v1 @ 21:5-21:24
                initializer
                  expr e1 method_call id -> fn id => nominal struct body_local_impl_fixture[lib]::crate::GlobalId @ 21:14-21:23
                    receiver
                      expr e0 path user -> local v0 => local nominal struct fn body_local_impl_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 21:14-21:18
              stmt s4 let v2 @ 22:5-22:30
                initializer
                  expr e3 method_call again -> fn again => local nominal struct fn body_local_impl_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 22:17-22:29
                    receiver
                      expr e2 path user -> local v0 => local nominal struct fn body_local_impl_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 22:17-22:21
        "#]],
    );
}
