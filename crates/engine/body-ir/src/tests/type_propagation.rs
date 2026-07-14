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

#[test]
fn preserves_instantiated_args_in_associated_projection_alias() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_alias_projection_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Iterator {
    type Item;
}

pub struct Iter<T>(T);
pub struct User;

impl<T> Iterator for Iter<T> {
    type Item = T;
}

pub type ItemOf<I: Iterator> = I::Item;

pub fn use_it(value: ItemOf<Iter<User>>) {
    value;
}
"#,
        expect![[r#"
            package body_alias_projection_fixture

            body_alias_projection_fixture [lib]
            body b0 fn body_alias_projection_fixture[lib]::crate::use_it @ 14:1-16:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 param value `value`: ItemOf<Iter<User>> => projection type trait body_alias_projection_fixture[lib]::crate::Iterator::Item<nominal struct body_alias_projection_fixture[lib]::crate::Iter<nominal struct body_alias_projection_fixture[lib]::crate::User>> @ 14:15-14:20
            body
            expr e1 block s1 => () @ 14:42-16:2
              stmt s0 expr; @ 15:5-15:11
                expr e0 path value -> local v0 => projection type trait body_alias_projection_fixture[lib]::crate::Iterator::Item<nominal struct body_alias_projection_fixture[lib]::crate::Iter<nominal struct body_alias_projection_fixture[lib]::crate::User>> @ 15:5-15:10
        "#]],
    );
}

#[test]
fn resolves_associated_type_used_by_another_bound() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_bound_projection_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait SchemaRead {
    type Dst;
}

pub trait IntoIterator {
    type Item;
}

pub trait FromIterator<T> {}

pub struct CollectionReader<Coll>(Coll);

impl<Coll> CollectionReader<Coll>
where
    Coll: IntoIterator<Item: SchemaRead>,
    Coll: FromIterator<<Coll::Item as SchemaRead>::Dst>,
{
    pub fn read(item: Coll::Item) {
        item;
    }
}
"#,
        expect![[r#"
            package body_bound_projection_fixture

            body_bound_projection_fixture [lib]
            body b0 fn impl CollectionReader<Coll>::read @ 18:5-20:6
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 param item `item`: Coll::Item => projection type trait body_bound_projection_fixture[lib]::crate::IntoIterator::Item<param Coll> @ 18:17-18:21
            body
            expr e1 block s1 => () @ 18:35-20:6
              stmt s0 expr; @ 19:9-19:14
                expr e0 path item -> local v0 => projection type trait body_bound_projection_fixture[lib]::crate::IntoIterator::Item<param Coll> @ 19:9-19:13
        "#]],
    );
}
