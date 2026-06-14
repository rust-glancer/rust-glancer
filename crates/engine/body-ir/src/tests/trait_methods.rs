use expect_test::expect;

use super::utils::check_project_body_ir;

#[test]
fn resolves_trait_methods_with_naive_applicability() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_trait_applicability_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Error;

pub struct Wrapper<T> {
    value: T,
}

pub trait DirectTrait {
    fn direct(&self) -> User;
}

impl DirectTrait for User {
    fn direct(&self) -> User {
        User
    }
}

pub trait GenericTrait {
    fn generic(&self) -> User;
}

impl<T> GenericTrait for Wrapper<T> {
    fn generic(&self) -> User {
        User
    }
}

pub trait UserOnlyTrait {
    fn user_only(&self) -> User;
}

impl UserOnlyTrait for Wrapper<User> {
    fn user_only(&self) -> User {
        User
    }
}

pub fn use_it(user: User, wrapper: Wrapper<Error>) {
    let direct = user.direct();
    let generic = wrapper.generic();
    let missing = wrapper.user_only();
}
"#,
        expect![[r#"
            package body_trait_applicability_fixture

            body_trait_applicability_fixture [lib]
            body b0 fn body_trait_applicability_fixture[lib]::crate::use_it @ 38:1-42:2
            scopes
            - s0 parent <none>: v0, v1
            - s1 parent s0: v2, v3, v4
            bindings
            - v0 param user `user`: User => nominal struct body_trait_applicability_fixture[lib]::crate::User @ 38:15-38:19
            - v1 param wrapper `wrapper`: Wrapper<Error> => nominal struct body_trait_applicability_fixture[lib]::crate::Wrapper<nominal struct body_trait_applicability_fixture[lib]::crate::Error> @ 38:27-38:34
            - v2 let direct `direct` => nominal struct body_trait_applicability_fixture[lib]::crate::User @ 39:9-39:15
            - v3 let generic `generic` => nominal struct body_trait_applicability_fixture[lib]::crate::User @ 40:9-40:16
            - v4 let missing `missing` => <unknown> @ 41:9-41:16
            body
            expr e6 block s1 => () @ 38:52-42:2
              stmt s0 let v2 @ 39:5-39:32
                initializer
                  expr e1 method_call direct -> fn trait body_trait_applicability_fixture[lib]::crate::DirectTrait::direct => nominal struct body_trait_applicability_fixture[lib]::crate::User @ 39:18-39:31
                    receiver
                      expr e0 path user -> local v0 => nominal struct body_trait_applicability_fixture[lib]::crate::User @ 39:18-39:22
              stmt s1 let v3 @ 40:5-40:37
                initializer
                  expr e3 method_call generic -> fn trait body_trait_applicability_fixture[lib]::crate::GenericTrait::generic => nominal struct body_trait_applicability_fixture[lib]::crate::User @ 40:19-40:36
                    receiver
                      expr e2 path wrapper -> local v1 => nominal struct body_trait_applicability_fixture[lib]::crate::Wrapper<nominal struct body_trait_applicability_fixture[lib]::crate::Error> @ 40:19-40:26
              stmt s2 let v4 @ 41:5-41:39
                initializer
                  expr e5 method_call user_only => <unknown> @ 41:19-41:38
                    receiver
                      expr e4 path wrapper -> local v1 => nominal struct body_trait_applicability_fixture[lib]::crate::Wrapper<nominal struct body_trait_applicability_fixture[lib]::crate::Error> @ 41:19-41:26


            body b1 fn impl DirectTrait for User::direct @ 13:5-15:6
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `&self` => &Self struct body_trait_applicability_fixture[lib]::crate::User @ 13:15-13:20
            body
            expr e1 block s1 => nominal struct body_trait_applicability_fixture[lib]::crate::User @ 13:30-15:6
              tail
                expr e0 path User -> item struct body_trait_applicability_fixture[lib]::crate::User => nominal struct body_trait_applicability_fixture[lib]::crate::User @ 14:9-14:13


            body b2 fn impl GenericTrait for Wrapper<T>::generic @ 23:5-25:6
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `&self` => &Self struct body_trait_applicability_fixture[lib]::crate::Wrapper<syntax T> @ 23:16-23:21
            body
            expr e1 block s1 => nominal struct body_trait_applicability_fixture[lib]::crate::User @ 23:31-25:6
              tail
                expr e0 path User -> item struct body_trait_applicability_fixture[lib]::crate::User => nominal struct body_trait_applicability_fixture[lib]::crate::User @ 24:9-24:13


            body b3 fn impl UserOnlyTrait for Wrapper<User>::user_only @ 33:5-35:6
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `&self` => &Self struct body_trait_applicability_fixture[lib]::crate::Wrapper<nominal struct body_trait_applicability_fixture[lib]::crate::User> @ 33:18-33:23
            body
            expr e1 block s1 => nominal struct body_trait_applicability_fixture[lib]::crate::User @ 33:33-35:6
              tail
                expr e0 path User -> item struct body_trait_applicability_fixture[lib]::crate::User => nominal struct body_trait_applicability_fixture[lib]::crate::User @ 34:9-34:13
        "#]],
    );
}

#[test]
fn method_lookup_excludes_traits_from_unrelated_workspace_targets() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/shared", "crates/app", "crates/other"]
resolver = "3"

//- /crates/shared/Cargo.toml
[package]
name = "shared"
version = "0.1.0"
edition = "2024"

//- /crates/shared/src/lib.rs
pub struct Maybe;

impl Maybe {
    pub fn is_some(&self) -> bool {
        true
    }
}

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
shared = { path = "../shared" }

//- /crates/app/src/lib.rs
use shared::Maybe;

pub fn use_it(maybe: Maybe) {
    let ok = maybe.is_some();
    let missing = maybe.and_then();
}

//- /crates/other/Cargo.toml
[package]
name = "other"
version = "0.1.0"
edition = "2024"

[dependencies]
shared = { path = "../shared" }

//- /crates/other/src/lib.rs
use shared::Maybe;

pub trait OtherExt {
    fn and_then(&self) -> bool;
}

impl OtherExt for Maybe {
    fn and_then(&self) -> bool {
        true
    }
}
"#,
        expect![[r#"
            package app

            app [lib]
            body b0 fn app[lib]::crate::use_it @ 3:1-6:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1, v2
            bindings
            - v0 param maybe `maybe`: Maybe => nominal struct shared[lib]::crate::Maybe @ 3:15-3:20
            - v1 let ok `ok` => bool @ 4:9-4:11
            - v2 let missing `missing` => <unknown> @ 5:9-5:16
            body
            expr e4 block s1 => () @ 3:29-6:2
              stmt s0 let v1 @ 4:5-4:30
                initializer
                  expr e1 method_call is_some -> fn impl Maybe::is_some => bool @ 4:14-4:29
                    receiver
                      expr e0 path maybe -> local v0 => nominal struct shared[lib]::crate::Maybe @ 4:14-4:19
              stmt s1 let v2 @ 5:5-5:36
                initializer
                  expr e3 method_call and_then => <unknown> @ 5:19-5:35
                    receiver
                      expr e2 path maybe -> local v0 => nominal struct shared[lib]::crate::Maybe @ 5:19-5:24


            package other

            other [lib]
            body b0 fn impl OtherExt for Maybe::and_then @ 8:5-10:6
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `&self` => &Self struct shared[lib]::crate::Maybe @ 8:17-8:22
            body
            expr e1 block s1 => bool @ 8:32-10:6
              tail
                expr e0 literal bool `true` => bool @ 9:9-9:13


            package shared

            shared [lib]
            body b0 fn impl Maybe::is_some @ 4:5-6:6
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `&self` => &Self struct shared[lib]::crate::Maybe @ 4:20-4:25
            body
            expr e1 block s1 => bool @ 4:35-6:6
              tail
                expr e0 literal bool `true` => bool @ 5:9-5:13
        "#]],
    );
}

#[test]
fn resolves_trait_associated_consts_for_qualified_value_paths() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_trait_assoc_const_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum StmtKind {
    Let,
}

pub struct Stmt;

pub trait HasAttrs {
    const SUPPORTS_CUSTOM_INNER_ATTRS: bool;
}

impl HasAttrs for StmtKind {
    const SUPPORTS_CUSTOM_INNER_ATTRS: bool = true;
}

impl HasAttrs for Stmt {
    const SUPPORTS_CUSTOM_INNER_ATTRS: bool = StmtKind::SUPPORTS_CUSTOM_INNER_ATTRS;
}

pub fn use_it() {
    let supports = StmtKind::SUPPORTS_CUSTOM_INNER_ATTRS;
    let qualified = <StmtKind as HasAttrs>::SUPPORTS_CUSTOM_INNER_ATTRS;
}
"#,
        expect![[r#"
            package body_trait_assoc_const_fixture

            body_trait_assoc_const_fixture [lib]
            body b0 fn body_trait_assoc_const_fixture[lib]::crate::use_it @ 19:1-22:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1
            bindings
            - v0 let supports `supports` => bool @ 20:9-20:17
            - v1 let qualified `qualified` => bool @ 21:9-21:18
            body
            expr e2 block s1 => () @ 19:17-22:2
              stmt s0 let v0 @ 20:5-20:58
                initializer
                  expr e0 path StmtKind::SUPPORTS_CUSTOM_INNER_ATTRS -> const impl HasAttrs for StmtKind::SUPPORTS_CUSTOM_INNER_ATTRS => bool @ 20:20-20:57
              stmt s1 let v1 @ 21:5-21:73
                initializer
                  expr e1 path <StmtKind as HasAttrs>::SUPPORTS_CUSTOM_INNER_ATTRS -> const impl HasAttrs for StmtKind::SUPPORTS_CUSTOM_INNER_ATTRS => bool @ 21:21-21:72


            body b1 const impl HasAttrs for StmtKind::SUPPORTS_CUSTOM_INNER_ATTRS @ 12:5-12:52
            scopes
            - s0 parent <none>: <none>
            bindings
            body
            expr e0 literal bool `true` => bool @ 12:47-12:51


            body b2 const impl HasAttrs for Stmt::SUPPORTS_CUSTOM_INNER_ATTRS @ 16:5-16:85
            scopes
            - s0 parent <none>: <none>
            bindings
            body
            expr e0 path StmtKind::SUPPORTS_CUSTOM_INNER_ATTRS -> const impl HasAttrs for StmtKind::SUPPORTS_CUSTOM_INNER_ATTRS => bool @ 16:47-16:84
        "#]],
    );
}
