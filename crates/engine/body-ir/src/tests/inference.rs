use expect_test::expect;

use super::utils::check_project_body_ir;

#[test]
fn records_simple_assignment_inference_facts() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_assignment_inference_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn missing<T>() -> T {}
pub fn id<T>(value: T) -> T {}

pub fn use_it(user: User) {
    let mut value = id(missing());
    value = user;
    value;
}
"#,
        expect![[r#"
            package body_assignment_inference_fixture

            body_assignment_inference_fixture [lib]
            body b0 fn body_assignment_inference_fixture[lib]::crate::missing @ 3:1-3:28
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e0 block s1 => () @ 3:26-3:28


            body b1 fn body_assignment_inference_fixture[lib]::crate::id @ 4:1-4:31
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 param value `value`: T => syntax T @ 4:14-4:19
            body
            expr e0 block s1 => () @ 4:29-4:31


            body b2 fn body_assignment_inference_fixture[lib]::crate::use_it @ 6:1-10:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1
            bindings
            - v0 param user `user`: User => nominal struct body_assignment_inference_fixture[lib]::crate::User @ 6:15-6:19
            - v1 let value `mut value` => nominal struct body_assignment_inference_fixture[lib]::crate::User @ 7:9-7:18 name @ 7:13-7:18
            body
            expr e8 block s1 => () @ 6:27-10:2
              stmt s0 let v1 @ 7:5-7:35
                initializer
                  expr e3 call => nominal struct body_assignment_inference_fixture[lib]::crate::User @ 7:21-7:34
                    callee
                      expr e0 path id -> fn body_assignment_inference_fixture[lib]::crate::id => <unknown> @ 7:21-7:23
                    arg
                      expr e2 call => nominal struct body_assignment_inference_fixture[lib]::crate::User @ 7:24-7:33
                        callee
                          expr e1 path missing -> fn body_assignment_inference_fixture[lib]::crate::missing => <unknown> @ 7:24-7:31
              stmt s1 expr; @ 8:5-8:18
                expr e6 assign = => () @ 8:5-8:17
                  target
                    expr e4 path value -> local v1 => nominal struct body_assignment_inference_fixture[lib]::crate::User @ 8:5-8:10
                  value
                    expr e5 path user -> local v0 => nominal struct body_assignment_inference_fixture[lib]::crate::User @ 8:13-8:17
              stmt s2 expr; @ 9:5-9:11
                expr e7 path value -> local v1 => nominal struct body_assignment_inference_fixture[lib]::crate::User @ 9:5-9:10
        "#]],
    );
}

#[test]
fn infers_imported_enum_variant_constructors() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_imported_enum_variant_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Option<T> {
    Some(T),
    None,
}

use Option::{Some, None};

pub fn use_it() {
    let value = Some(10u8);
    let absent: Option<u8> = None;
    value;
    absent;
}
"#,
        expect![[r#"
            package body_imported_enum_variant_inference

            body_imported_enum_variant_inference [lib]
            body b0 fn body_imported_enum_variant_inference[lib]::crate::use_it @ 8:1-13:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1
            bindings
            - v0 let value `value` => nominal enum body_imported_enum_variant_inference[lib]::crate::Option<u8> @ 9:9-9:14
            - v1 let absent `absent`: Option<u8> => nominal enum body_imported_enum_variant_inference[lib]::crate::Option<u8> @ 10:9-10:15
            body
            expr e6 block s1 => () @ 8:17-13:2
              stmt s0 let v0 @ 9:5-9:28
                initializer
                  expr e2 call => nominal enum body_imported_enum_variant_inference[lib]::crate::Option<u8> @ 9:17-9:27
                    callee
                      expr e0 path Some -> variant enum body_imported_enum_variant_inference[lib]::crate::Option::Some => nominal enum body_imported_enum_variant_inference[lib]::crate::Option<<unknown>> @ 9:17-9:21
                    arg
                      expr e1 literal int `10u8` => u8 @ 9:22-9:26
              stmt s1 let v1: Option<u8> @ 10:5-10:35
                initializer
                  expr e3 path None -> variant enum body_imported_enum_variant_inference[lib]::crate::Option::None => nominal enum body_imported_enum_variant_inference[lib]::crate::Option<<unknown>> @ 10:30-10:34
              stmt s2 expr; @ 11:5-11:11
                expr e4 path value -> local v0 => nominal enum body_imported_enum_variant_inference[lib]::crate::Option<u8> @ 11:5-11:10
              stmt s3 expr; @ 12:5-12:12
                expr e5 path absent -> local v1 => nominal enum body_imported_enum_variant_inference[lib]::crate::Option<u8> @ 12:5-12:11
        "#]],
    );
}

#[test]
fn infers_static_associated_function_impl_generics_from_args() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_static_assoc_fn_impl_generic_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub struct Vec<T> {
    value: T,
}

impl<T> Vec<T> {
    pub fn singleton(value: T) -> Self {}
}

pub fn use_it(user: User) {
    let singleton = Vec::singleton(user);
    singleton;
}
"#,
        expect![[r#"
            package body_static_assoc_fn_impl_generic_inference

            body_static_assoc_fn_impl_generic_inference [lib]
            body b0 fn body_static_assoc_fn_impl_generic_inference[lib]::crate::use_it @ 11:1-14:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1
            bindings
            - v0 param user `user`: User => nominal struct body_static_assoc_fn_impl_generic_inference[lib]::crate::User @ 11:15-11:19
            - v1 let singleton `singleton` => nominal struct body_static_assoc_fn_impl_generic_inference[lib]::crate::Vec<nominal struct body_static_assoc_fn_impl_generic_inference[lib]::crate::User> @ 12:9-12:18
            body
            expr e4 block s1 => () @ 11:27-14:2
              stmt s0 let v1 @ 12:5-12:42
                initializer
                  expr e2 call => nominal struct body_static_assoc_fn_impl_generic_inference[lib]::crate::Vec<nominal struct body_static_assoc_fn_impl_generic_inference[lib]::crate::User> @ 12:21-12:41
                    callee
                      expr e0 path Vec::singleton -> fn impl Vec<T>::singleton => <unknown> @ 12:21-12:35
                    arg
                      expr e1 path user -> local v0 => nominal struct body_static_assoc_fn_impl_generic_inference[lib]::crate::User @ 12:36-12:40
              stmt s1 expr; @ 13:5-13:15
                expr e3 path singleton -> local v1 => nominal struct body_static_assoc_fn_impl_generic_inference[lib]::crate::Vec<nominal struct body_static_assoc_fn_impl_generic_inference[lib]::crate::User> @ 13:5-13:14


            body b1 fn impl Vec<T>::singleton @ 8:5-8:42
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 param value `value`: T => syntax T @ 8:22-8:27
            body
            expr e0 block s1 => () @ 8:40-8:42
        "#]],
    );
}

#[test]
fn infers_collect_destination_from_selected_trait_obligations() {
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
    pub trait FromIterator<A> {}

    pub trait Iterator {
        type Item;

        fn collect<B>(self) -> B
        where
            B: FromIterator<Self::Item>;
    }
}

pub mod slice {
    pub struct Iter<'a, T>(&'a T);
}

pub struct Vec<T> {
    value: T,
}

impl<T> iter::FromIterator<T> for Vec<T> {}

impl<T> [T] {
    pub fn iter(&self) -> slice::Iter<'_, T> {
        missing()
    }
}

impl<'a, T> iter::Iterator for slice::Iter<'a, T> {
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
use core::Vec;
use storage::DefMap;

pub fn explicit_destination(def_map: &DefMap) {
    let imports = def_map.imports().iter().collect::<Vec<_>>();
    imports;
}

pub fn expected_destination(def_map: &DefMap) {
    let imports: Vec<_> = def_map.imports().iter().collect();
    imports;
}
"#,
        expect![[r#"
            package app

            app [lib]
            body b0 fn app[lib]::crate::explicit_destination @ 4:1-7:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1
            bindings
            - v0 param def_map `def_map`: &DefMap => &nominal struct storage[lib]::crate::DefMap @ 4:29-4:36
            - v1 let imports `imports` => nominal struct fake_core[lib]::crate::Vec<&nominal struct storage[lib]::crate::ImportData> @ 5:9-5:16
            body
            expr e5 block s1 => () @ 4:47-7:2
              stmt s0 let v1 @ 5:5-5:64
                initializer
                  expr e3 method_call collect<Vec<_>> -> fn trait fake_core[lib]::crate::iter::Iterator::collect => nominal struct fake_core[lib]::crate::Vec<&nominal struct storage[lib]::crate::ImportData> @ 5:19-5:63
                    receiver
                      expr e2 method_call iter -> fn impl [T]::iter => nominal struct fake_core[lib]::crate::slice::Iter<'_, nominal struct storage[lib]::crate::ImportData> @ 5:19-5:43
                        receiver
                          expr e1 method_call imports -> fn impl DefMap::imports => &[nominal struct storage[lib]::crate::ImportData] @ 5:19-5:36
                            receiver
                              expr e0 path def_map -> local v0 => &nominal struct storage[lib]::crate::DefMap @ 5:19-5:26
              stmt s1 expr; @ 6:5-6:13
                expr e4 path imports -> local v1 => nominal struct fake_core[lib]::crate::Vec<&nominal struct storage[lib]::crate::ImportData> @ 6:5-6:12


            body b1 fn app[lib]::crate::expected_destination @ 9:1-12:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1
            bindings
            - v0 param def_map `def_map`: &DefMap => &nominal struct storage[lib]::crate::DefMap @ 9:29-9:36
            - v1 let imports `imports`: Vec<_> => nominal struct fake_core[lib]::crate::Vec<&nominal struct storage[lib]::crate::ImportData> @ 10:9-10:16
            body
            expr e5 block s1 => () @ 9:47-12:2
              stmt s0 let v1: Vec<_> @ 10:5-10:62
                initializer
                  expr e3 method_call collect -> fn trait fake_core[lib]::crate::iter::Iterator::collect => nominal struct fake_core[lib]::crate::Vec<&nominal struct storage[lib]::crate::ImportData> @ 10:27-10:61
                    receiver
                      expr e2 method_call iter -> fn impl [T]::iter => nominal struct fake_core[lib]::crate::slice::Iter<'_, nominal struct storage[lib]::crate::ImportData> @ 10:27-10:51
                        receiver
                          expr e1 method_call imports -> fn impl DefMap::imports => &[nominal struct storage[lib]::crate::ImportData] @ 10:27-10:44
                            receiver
                              expr e0 path def_map -> local v0 => &nominal struct storage[lib]::crate::DefMap @ 10:27-10:34
              stmt s1 expr; @ 11:5-11:13
                expr e4 path imports -> local v1 => nominal struct fake_core[lib]::crate::Vec<&nominal struct storage[lib]::crate::ImportData> @ 11:5-11:12


            package fake_core

            fake_core [lib]
            body b0 fn impl [T]::iter @ 24:5-26:6
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `&self` => <unknown> @ 24:17-24:22
            body
            expr e2 block s1 => <unknown> @ 24:46-26:6
              tail
                expr e1 call => <unknown> @ 25:9-25:18
                  callee
                    expr e0 path missing => <unknown> @ 25:9-25:16


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
fn keeps_trait_obligation_solving_conservative() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_conservative_trait_obligation_solving"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub struct Vec<T> {
    value: T,
}

pub trait FromIterator<A> {}

pub struct Iter<T> {
    value: T,
}

impl<T> Iter<T> {
    pub fn collect<B>(self) -> B
    where
        B: FromIterator<T>,
    {
        missing()
    }
}

pub struct NotIterator<T> {
    value: T,
}

impl<T> NotIterator<T> {
    pub fn collect<B>(self) -> B {
        missing()
    }
}

impl<T> FromIterator<T> for Vec<T> {}
impl FromIterator<User> for Vec<User> {}

pub fn missing<T>() -> T {}

pub fn ambiguous_impls(iter: Iter<User>) {
    let collected = iter.collect::<Vec<_>>();
    collected;
}

pub fn unrelated_collect(iter: NotIterator<User>) {
    let collected = iter.collect::<Vec<_>>();
    collected;
}
"#,
        expect![[r#"
            package body_conservative_trait_obligation_solving

            body_conservative_trait_obligation_solving [lib]
            body b0 fn body_conservative_trait_obligation_solving[lib]::crate::missing @ 35:1-35:28
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e0 block s1 => () @ 35:26-35:28


            body b1 fn body_conservative_trait_obligation_solving[lib]::crate::ambiguous_impls @ 37:1-40:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1
            bindings
            - v0 param iter `iter`: Iter<User> => nominal struct body_conservative_trait_obligation_solving[lib]::crate::Iter<nominal struct body_conservative_trait_obligation_solving[lib]::crate::User> @ 37:24-37:28
            - v1 let collected `collected` => nominal struct body_conservative_trait_obligation_solving[lib]::crate::Vec<<unknown>> @ 38:9-38:18
            body
            expr e3 block s1 => () @ 37:42-40:2
              stmt s0 let v1 @ 38:5-38:46
                initializer
                  expr e1 method_call collect<Vec<_>> -> fn impl Iter<T>::collect => nominal struct body_conservative_trait_obligation_solving[lib]::crate::Vec<<unknown>> @ 38:21-38:45
                    receiver
                      expr e0 path iter -> local v0 => nominal struct body_conservative_trait_obligation_solving[lib]::crate::Iter<nominal struct body_conservative_trait_obligation_solving[lib]::crate::User> @ 38:21-38:25
              stmt s1 expr; @ 39:5-39:15
                expr e2 path collected -> local v1 => nominal struct body_conservative_trait_obligation_solving[lib]::crate::Vec<<unknown>> @ 39:5-39:14


            body b2 fn body_conservative_trait_obligation_solving[lib]::crate::unrelated_collect @ 42:1-45:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1
            bindings
            - v0 param iter `iter`: NotIterator<User> => nominal struct body_conservative_trait_obligation_solving[lib]::crate::NotIterator<nominal struct body_conservative_trait_obligation_solving[lib]::crate::User> @ 42:26-42:30
            - v1 let collected `collected` => nominal struct body_conservative_trait_obligation_solving[lib]::crate::Vec<<unknown>> @ 43:9-43:18
            body
            expr e3 block s1 => () @ 42:51-45:2
              stmt s0 let v1 @ 43:5-43:46
                initializer
                  expr e1 method_call collect<Vec<_>> -> fn impl NotIterator<T>::collect => nominal struct body_conservative_trait_obligation_solving[lib]::crate::Vec<<unknown>> @ 43:21-43:45
                    receiver
                      expr e0 path iter -> local v0 => nominal struct body_conservative_trait_obligation_solving[lib]::crate::NotIterator<nominal struct body_conservative_trait_obligation_solving[lib]::crate::User> @ 43:21-43:25
              stmt s1 expr; @ 44:5-44:15
                expr e2 path collected -> local v1 => nominal struct body_conservative_trait_obligation_solving[lib]::crate::Vec<<unknown>> @ 44:5-44:14


            body b3 fn impl Iter<T>::collect @ 14:5-19:6
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `self` => Self struct body_conservative_trait_obligation_solving[lib]::crate::Iter<syntax T> @ 14:23-14:27
            body
            expr e2 block s1 => <unknown> @ 17:5-19:6
              tail
                expr e1 call => <unknown> @ 18:9-18:18
                  callee
                    expr e0 path missing -> fn body_conservative_trait_obligation_solving[lib]::crate::missing => <unknown> @ 18:9-18:16


            body b4 fn impl NotIterator<T>::collect @ 27:5-29:6
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `self` => Self struct body_conservative_trait_obligation_solving[lib]::crate::NotIterator<syntax T> @ 27:23-27:27
            body
            expr e2 block s1 => <unknown> @ 27:34-29:6
              tail
                expr e1 call => <unknown> @ 28:9-28:18
                  callee
                    expr e0 path missing -> fn body_conservative_trait_obligation_solving[lib]::crate::missing => <unknown> @ 28:9-28:16
        "#]],
    );
}

#[test]
fn ignores_never_branches_when_inferring_common_result() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_never_branch_result_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn use_it(flag: bool, user: User) -> User {
    let value = if flag {
        return user;
    } else {
        user
    };
    value
}
"#,
        expect![[r#"
            package body_never_branch_result_inference

            body_never_branch_result_inference [lib]
            body b0 fn body_never_branch_result_inference[lib]::crate::use_it @ 3:1-10:2
            scopes
            - s0 parent <none>: v0, v1
            - s1 parent s0: v2
            - s2 parent s1: <none>
            - s3 parent s1: <none>
            bindings
            - v0 param flag `flag`: bool => bool @ 3:15-3:19
            - v1 param user `user`: User => nominal struct body_never_branch_result_inference[lib]::crate::User @ 3:27-3:31
            - v2 let value `value` => nominal struct body_never_branch_result_inference[lib]::crate::User @ 4:9-4:14
            body
            expr e8 block s1 => nominal struct body_never_branch_result_inference[lib]::crate::User @ 3:47-10:2
              stmt s1 let v2 @ 4:5-8:7
                initializer
                  expr e6 if => nominal struct body_never_branch_result_inference[lib]::crate::User @ 4:17-8:6
                    condition
                      expr e0 path flag -> local v0 => bool @ 4:20-4:24
                    then
                      expr e3 block s2 => ! @ 4:25-6:6
                        stmt s0 expr; @ 5:9-5:21
                          expr e2 wrapper return => ! @ 5:9-5:20
                            inner
                              expr e1 path user -> local v1 => nominal struct body_never_branch_result_inference[lib]::crate::User @ 5:16-5:20
                    else
                      expr e5 block s3 => nominal struct body_never_branch_result_inference[lib]::crate::User @ 6:12-8:6
                        tail
                          expr e4 path user -> local v1 => nominal struct body_never_branch_result_inference[lib]::crate::User @ 7:9-7:13
              tail
                expr e7 path value -> local v2 => nominal struct body_never_branch_result_inference[lib]::crate::User @ 9:5-9:10
        "#]],
    );
}

#[test]
fn self_referential_if_result_does_not_recurse() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_self_referential_if_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Maybe<T> {
    Some(T),
    None,
}

pub enum Mode {
    First,
    Second,
}

pub fn use_it(flag: bool) {
    let mut value = Maybe::Some(Mode::First);
    value = if flag {
        value
    } else {
        Maybe::Some(Mode::Second)
    };
    value;
}
"#,
        expect![[r#"
            package body_self_referential_if_inference

            body_self_referential_if_inference [lib]
            body b0 fn body_self_referential_if_inference[lib]::crate::use_it @ 11:1-19:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1
            - s2 parent s1: <none>
            - s3 parent s1: <none>
            bindings
            - v0 param flag `flag`: bool => bool @ 11:15-11:19
            - v1 let value `mut value` => nominal enum body_self_referential_if_inference[lib]::crate::Maybe<nominal enum body_self_referential_if_inference[lib]::crate::Mode> @ 12:9-12:18 name @ 12:13-12:18
            body
            expr e14 block s1 => () @ 11:27-19:2
              stmt s0 let v1 @ 12:5-12:46
                initializer
                  expr e2 call => nominal enum body_self_referential_if_inference[lib]::crate::Maybe<nominal enum body_self_referential_if_inference[lib]::crate::Mode> @ 12:21-12:45
                    callee
                      expr e0 path Maybe::Some -> variant enum body_self_referential_if_inference[lib]::crate::Maybe::Some => nominal enum body_self_referential_if_inference[lib]::crate::Maybe<<unknown>> @ 12:21-12:32
                    arg
                      expr e1 path Mode::First -> variant enum body_self_referential_if_inference[lib]::crate::Mode::First => nominal enum body_self_referential_if_inference[lib]::crate::Mode @ 12:33-12:44
              stmt s1 expr; @ 13:5-17:7
                expr e12 assign = => () @ 13:5-17:6
                  target
                    expr e3 path value -> local v1 => nominal enum body_self_referential_if_inference[lib]::crate::Maybe<nominal enum body_self_referential_if_inference[lib]::crate::Mode> @ 13:5-13:10
                  value
                    expr e11 if => nominal enum body_self_referential_if_inference[lib]::crate::Maybe<nominal enum body_self_referential_if_inference[lib]::crate::Mode> @ 13:13-17:6
                      condition
                        expr e4 path flag -> local v0 => bool @ 13:16-13:20
                      then
                        expr e6 block s2 => nominal enum body_self_referential_if_inference[lib]::crate::Maybe<nominal enum body_self_referential_if_inference[lib]::crate::Mode> @ 13:21-15:6
                          tail
                            expr e5 path value -> local v1 => nominal enum body_self_referential_if_inference[lib]::crate::Maybe<nominal enum body_self_referential_if_inference[lib]::crate::Mode> @ 14:9-14:14
                      else
                        expr e10 block s3 => nominal enum body_self_referential_if_inference[lib]::crate::Maybe<nominal enum body_self_referential_if_inference[lib]::crate::Mode> @ 15:12-17:6
                          tail
                            expr e9 call => nominal enum body_self_referential_if_inference[lib]::crate::Maybe<nominal enum body_self_referential_if_inference[lib]::crate::Mode> @ 16:9-16:34
                              callee
                                expr e7 path Maybe::Some -> variant enum body_self_referential_if_inference[lib]::crate::Maybe::Some => nominal enum body_self_referential_if_inference[lib]::crate::Maybe<<unknown>> @ 16:9-16:20
                              arg
                                expr e8 path Mode::Second -> variant enum body_self_referential_if_inference[lib]::crate::Mode::Second => nominal enum body_self_referential_if_inference[lib]::crate::Mode @ 16:21-16:33
              stmt s2 expr; @ 18:5-18:11
                expr e13 path value -> local v1 => nominal enum body_self_referential_if_inference[lib]::crate::Maybe<nominal enum body_self_referential_if_inference[lib]::crate::Mode> @ 18:5-18:10
        "#]],
    );
}

#[test]
fn self_referential_match_result_does_not_recurse() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_self_referential_match_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Maybe<T> {
    Some(T),
    None,
}

pub enum MergeBehavior {
    Crate,
    Module,
}

pub enum Guess {
    Keep,
    Clear,
    Nested,
}

pub fn use_it(guess: Guess) {
    let mut mb = Maybe::Some(MergeBehavior::Crate);
    mb = match guess {
        Guess::Keep => mb,
        Guess::Clear => Maybe::None,
        Guess::Nested => match mb {
            Maybe::Some(_) => mb,
            Maybe::None => Maybe::Some(MergeBehavior::Module),
        },
    };
    mb;
}
"#,
        expect![[r#"
            package body_self_referential_match_inference

            body_self_referential_match_inference [lib]
            body b0 fn body_self_referential_match_inference[lib]::crate::use_it @ 17:1-28:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1
            - s2 parent s1: <none>
            - s3 parent s1: <none>
            - s4 parent s1: <none>
            - s5 parent s4: <none>
            - s6 parent s4: <none>
            bindings
            - v0 param guess `guess`: Guess => nominal enum body_self_referential_match_inference[lib]::crate::Guess @ 17:15-17:20
            - v1 let mb `mut mb` => nominal enum body_self_referential_match_inference[lib]::crate::Maybe<nominal enum body_self_referential_match_inference[lib]::crate::MergeBehavior> @ 18:9-18:15 name @ 18:13-18:15
            body
            expr e16 block s1 => () @ 17:29-28:2
              stmt s0 let v1 @ 18:5-18:52
                initializer
                  expr e2 call => nominal enum body_self_referential_match_inference[lib]::crate::Maybe<nominal enum body_self_referential_match_inference[lib]::crate::MergeBehavior> @ 18:18-18:51
                    callee
                      expr e0 path Maybe::Some -> variant enum body_self_referential_match_inference[lib]::crate::Maybe::Some => nominal enum body_self_referential_match_inference[lib]::crate::Maybe<<unknown>> @ 18:18-18:29
                    arg
                      expr e1 path MergeBehavior::Crate -> variant enum body_self_referential_match_inference[lib]::crate::MergeBehavior::Crate => nominal enum body_self_referential_match_inference[lib]::crate::MergeBehavior @ 18:30-18:50
              stmt s1 expr; @ 19:5-26:7
                expr e14 assign = => () @ 19:5-26:6
                  target
                    expr e3 path mb -> local v1 => nominal enum body_self_referential_match_inference[lib]::crate::Maybe<nominal enum body_self_referential_match_inference[lib]::crate::MergeBehavior> @ 19:5-19:7
                  value
                    expr e13 match => nominal enum body_self_referential_match_inference[lib]::crate::Maybe<nominal enum body_self_referential_match_inference[lib]::crate::MergeBehavior> @ 19:10-26:6
                      scrutinee
                        expr e4 path guess -> local v0 => nominal enum body_self_referential_match_inference[lib]::crate::Guess @ 19:16-19:21
                      arm s2
                        expr e5 path mb -> local v1 => nominal enum body_self_referential_match_inference[lib]::crate::Maybe<nominal enum body_self_referential_match_inference[lib]::crate::MergeBehavior> @ 20:24-20:26
                      arm s3
                        expr e6 path Maybe::None -> variant enum body_self_referential_match_inference[lib]::crate::Maybe::None => nominal enum body_self_referential_match_inference[lib]::crate::Maybe<<unknown>> @ 21:25-21:36
                      arm s4
                        expr e12 match => nominal enum body_self_referential_match_inference[lib]::crate::Maybe<nominal enum body_self_referential_match_inference[lib]::crate::MergeBehavior> @ 22:26-25:10
                          scrutinee
                            expr e7 path mb -> local v1 => nominal enum body_self_referential_match_inference[lib]::crate::Maybe<nominal enum body_self_referential_match_inference[lib]::crate::MergeBehavior> @ 22:32-22:34
                          arm s5
                            expr e8 path mb -> local v1 => nominal enum body_self_referential_match_inference[lib]::crate::Maybe<nominal enum body_self_referential_match_inference[lib]::crate::MergeBehavior> @ 23:31-23:33
                          arm s6
                            expr e11 call => nominal enum body_self_referential_match_inference[lib]::crate::Maybe<nominal enum body_self_referential_match_inference[lib]::crate::MergeBehavior> @ 24:28-24:62
                              callee
                                expr e9 path Maybe::Some -> variant enum body_self_referential_match_inference[lib]::crate::Maybe::Some => nominal enum body_self_referential_match_inference[lib]::crate::Maybe<<unknown>> @ 24:28-24:39
                              arg
                                expr e10 path MergeBehavior::Module -> variant enum body_self_referential_match_inference[lib]::crate::MergeBehavior::Module => nominal enum body_self_referential_match_inference[lib]::crate::MergeBehavior @ 24:40-24:61
              stmt s2 expr; @ 27:5-27:8
                expr e15 path mb -> local v1 => nominal enum body_self_referential_match_inference[lib]::crate::Maybe<nominal enum body_self_referential_match_inference[lib]::crate::MergeBehavior> @ 27:5-27:7
        "#]],
    );
}
