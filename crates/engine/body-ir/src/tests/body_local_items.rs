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
            - s1 parent s0: v0, v2; source_items i0
            - s2 parent s1: v1; source_items i1
            source_items
            - i0 struct User @ 4:5-4:17
            - i1 struct User @ 7:9-7:21
            bindings
            - v0 let local `local`: User => nominal struct fn body_local_item_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 5:9-5:14
            - v1 let nested `nested`: User => nominal struct fn body_local_item_fixture[lib]::crate::use_it::User @ 7:9-7:21 @ 8:13-8:19
            - v2 let again `again`: User => nominal struct fn body_local_item_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 10:9-10:14
            body
            expr e4 block s1 => () @ 3:17-11:2
              stmt s0 source_item i0 @ 4:5-4:17
              stmt s1 let v0: User @ 5:5-5:28
                initializer
                  expr e0 path User -> struct fn body_local_item_fixture[lib]::crate::use_it::User @ 4:5-4:17 => nominal struct fn body_local_item_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 5:23-5:27
              stmt s4 expr @ 6:5-9:6
                expr e2 block s2 => () @ 6:5-9:6
                  stmt s2 source_item i1 @ 7:9-7:21
                  stmt s3 let v1: User @ 8:9-8:33
                    initializer
                      expr e1 path User -> struct fn body_local_item_fixture[lib]::crate::use_it::User @ 7:9-7:21 => nominal struct fn body_local_item_fixture[lib]::crate::use_it::User @ 7:9-7:21 @ 8:28-8:32
              stmt s5 let v2: User @ 10:5-10:28
                initializer
                  expr e3 path User -> struct fn body_local_item_fixture[lib]::crate::use_it::User @ 4:5-4:17 => nominal struct fn body_local_item_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 10:23-10:27
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
            - s1 parent s0: v0, v1, v2; source_items i0, i1
            source_items
            - i0 struct User @ 4:5-7:6
            - i1 struct Pair @ 8:5-8:37
            bindings
            - v0 let user `user`: User => nominal struct fn body_local_field_fixture[lib]::crate::use_it::User @ 4:5-7:6 @ 10:9-10:13
            - v1 let id `id` => nominal struct body_local_field_fixture[lib]::crate::GlobalId @ 11:9-11:11
            - v2 let right `right` => nominal struct body_local_field_fixture[lib]::crate::GlobalId @ 12:9-12:14
            body
            expr e5 block s1 => () @ 3:17-13:2
              stmt s0 source_item i0 @ 4:5-7:6
              stmt s1 source_item i1 @ 8:5-8:37
              stmt s2 let v0: User @ 10:5-10:20
              stmt s3 let v1 @ 11:5-11:22
                initializer
                  expr e1 field id -> field struct fn body_local_field_fixture[lib]::crate::use_it::User @ 4:5-7:6::id => nominal struct body_local_field_fixture[lib]::crate::GlobalId @ 11:14-11:21
                    base
                      expr e0 path user -> local v0 => nominal struct fn body_local_field_fixture[lib]::crate::use_it::User @ 4:5-7:6 @ 11:14-11:18
              stmt s4 let v2 @ 12:5-12:29
                initializer
                  expr e4 field 1 -> field struct fn body_local_field_fixture[lib]::crate::use_it::Pair @ 8:5-8:37::#1 => nominal struct body_local_field_fixture[lib]::crate::GlobalId @ 12:17-12:28
                    base
                      expr e3 field pair -> field struct fn body_local_field_fixture[lib]::crate::use_it::User @ 4:5-7:6::pair => nominal struct fn body_local_field_fixture[lib]::crate::use_it::Pair @ 8:5-8:37 @ 12:17-12:26
                        base
                          expr e2 path user -> local v0 => nominal struct fn body_local_field_fixture[lib]::crate::use_it::User @ 4:5-7:6 @ 12:17-12:21
        "#]],
    );
}

#[test]
fn resolves_body_local_record_literals() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_local_record_literal_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User {
        id: GlobalId,
    }
    union Bits {
        id: GlobalId,
    }

    let user = User { id: GlobalId };
    let bits = Bits { id: GlobalId };
}
"#,
        expect![[r#"
            package body_local_record_literal_fixture

            body_local_record_literal_fixture [lib]
            body b0 fn body_local_record_literal_fixture[lib]::crate::use_it @ 3:1-13:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1; source_items i0, i1
            source_items
            - i0 struct User @ 4:5-6:6
            - i1 union Bits @ 7:5-9:6
            bindings
            - v0 let user `user` => nominal struct fn body_local_record_literal_fixture[lib]::crate::use_it::User @ 4:5-6:6 @ 11:9-11:13
            - v1 let bits `bits` => nominal union fn body_local_record_literal_fixture[lib]::crate::use_it::Bits @ 7:5-9:6 @ 12:9-12:13
            body
            expr e4 block s1 => () @ 3:17-13:2
              stmt s0 source_item i0 @ 4:5-6:6
              stmt s1 source_item i1 @ 7:5-9:6
              stmt s2 let v0 @ 11:5-11:38
                initializer
                  expr e1 record User -> struct fn body_local_record_literal_fixture[lib]::crate::use_it::User @ 4:5-6:6 => nominal struct fn body_local_record_literal_fixture[lib]::crate::use_it::User @ 4:5-6:6 @ 11:16-11:37
                    field id
                      expr e0 path GlobalId -> item struct body_local_record_literal_fixture[lib]::crate::GlobalId => nominal struct body_local_record_literal_fixture[lib]::crate::GlobalId @ 11:27-11:35
              stmt s3 let v1 @ 12:5-12:38
                initializer
                  expr e3 record Bits -> union fn body_local_record_literal_fixture[lib]::crate::use_it::Bits @ 7:5-9:6 => nominal union fn body_local_record_literal_fixture[lib]::crate::use_it::Bits @ 7:5-9:6 @ 12:16-12:37
                    field id
                      expr e2 path GlobalId -> item struct body_local_record_literal_fixture[lib]::crate::GlobalId => nominal struct body_local_record_literal_fixture[lib]::crate::GlobalId @ 12:27-12:35
        "#]],
    );
}

#[test]
fn substitutes_generic_body_local_type_alias_arguments() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_local_generic_alias_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Slot<T> {
    value: T,
}

pub fn use_it() {
    struct User;
    type Alias<T> = Slot<T>;

    let slot: Alias<User>;
    let value = slot.value;
}
"#,
        expect![[r#"
            package body_local_generic_alias_fixture

            body_local_generic_alias_fixture [lib]
            body b0 fn body_local_generic_alias_fixture[lib]::crate::use_it @ 5:1-11:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1; source_items i0, i1
            source_items
            - i0 struct User @ 6:5-6:17
            - i1 type_alias Alias @ 7:5-7:29
            bindings
            - v0 let slot `slot`: Alias<User> => nominal struct body_local_generic_alias_fixture[lib]::crate::Slot<nominal struct fn body_local_generic_alias_fixture[lib]::crate::use_it::User @ 6:5-6:17> @ 9:9-9:13
            - v1 let value `value` => nominal struct fn body_local_generic_alias_fixture[lib]::crate::use_it::User @ 6:5-6:17 @ 10:9-10:14
            body
            expr e2 block s1 => () @ 5:17-11:2
              stmt s0 source_item i0 @ 6:5-6:17
              stmt s1 source_item i1 @ 7:5-7:29
              stmt s2 let v0: Alias<User> @ 9:5-9:27
              stmt s3 let v1 @ 10:5-10:28
                initializer
                  expr e1 field value -> field struct body_local_generic_alias_fixture[lib]::crate::Slot::value => nominal struct fn body_local_generic_alias_fixture[lib]::crate::use_it::User @ 6:5-6:17 @ 10:17-10:27
                    base
                      expr e0 path slot -> local v0 => nominal struct body_local_generic_alias_fixture[lib]::crate::Slot<nominal struct fn body_local_generic_alias_fixture[lib]::crate::use_it::User @ 6:5-6:17> @ 10:17-10:21
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
            - s1 parent s0: v0, v1, v2; source_items i0, i4
            source_items
            - i0 struct User @ 4:5-4:17
            - i1 fn id @ 7:9-9:10
            - i2 fn again @ 11:9-13:10
            - i3 fn associated @ 15:9-17:10
            - i4 impl <unnamed> @ 6:5-18:6
            bindings
            - v0 let user `user`: User => nominal struct fn body_local_impl_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 20:9-20:13
            - v1 let id `id` => nominal struct body_local_impl_fixture[lib]::crate::GlobalId @ 21:9-21:11
            - v2 let again `again` => nominal struct fn body_local_impl_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 22:9-22:14
            body
            expr e4 block s1 => () @ 3:17-23:2
              stmt s0 source_item i0 @ 4:5-4:17
              stmt s1 source_item i4 @ 6:5-18:6
              stmt s2 let v0: User @ 20:5-20:20
              stmt s3 let v1 @ 21:5-21:24
                initializer
                  expr e1 method_call id -> fn impl User::id => nominal struct body_local_impl_fixture[lib]::crate::GlobalId @ 21:14-21:23
                    receiver
                      expr e0 path user -> local v0 => nominal struct fn body_local_impl_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 21:14-21:18
              stmt s4 let v2 @ 22:5-22:30
                initializer
                  expr e3 method_call again -> fn impl User::again => nominal struct fn body_local_impl_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 22:17-22:29
                    receiver
                      expr e2 path user -> local v0 => nominal struct fn body_local_impl_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 22:17-22:21
        "#]],
    );
}

#[test]
fn lowers_more_body_local_item_kinds() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_more_local_items_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    enum Action {
        Start(GlobalId),
        Stop,
    }
    union Bits {
        id: GlobalId,
    }
    type Alias = GlobalId;
    trait Named {}
    const DEFAULT: Alias = GlobalId;
    static mut CURRENT: GlobalId = GlobalId;
    fn helper() -> Alias {
        GlobalId
    }

    impl Action {
        const NAME: Alias = GlobalId;
        type Output = Alias;
        fn build() -> Alias {
            helper()
        }
    }

    let alias: Alias = helper();
    let default = DEFAULT;
    let current = CURRENT;
    let action = Action::Start(GlobalId);
}
"#,
        expect![[r#"
            package body_more_local_items_fixture

            body_more_local_items_fixture [lib]
            body b0 fn body_more_local_items_fixture[lib]::crate::use_it @ 3:1-31:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1, v2, v3; source_items i0, i1, i2, i3, i4, i5, i6, i10
            source_items
            - i0 enum Action @ 4:5-7:6
            - i1 union Bits @ 8:5-10:6
            - i2 type_alias Alias @ 11:5-11:27
            - i3 trait Named @ 12:5-12:19
            - i4 const DEFAULT @ 13:5-13:37
            - i5 static CURRENT @ 14:5-14:45
            - i6 fn helper @ 15:5-17:6
            - i7 const NAME @ 20:9-20:38
            - i8 type_alias Output @ 21:9-21:29
            - i9 fn build @ 22:9-24:10
            - i10 impl <unnamed> @ 19:5-25:6
            bindings
            - v0 let alias `alias`: Alias => nominal struct body_more_local_items_fixture[lib]::crate::GlobalId @ 27:9-27:14
            - v1 let default `default` => nominal struct body_more_local_items_fixture[lib]::crate::GlobalId @ 28:9-28:16
            - v2 let current `current` => nominal struct body_more_local_items_fixture[lib]::crate::GlobalId @ 29:9-29:16
            - v3 let action `action` => nominal enum fn body_more_local_items_fixture[lib]::crate::use_it::Action @ 4:5-7:6 @ 30:9-30:15
            body
            expr e7 block s1 => () @ 3:17-31:2
              stmt s0 source_item i0 @ 4:5-7:6
              stmt s1 source_item i1 @ 8:5-10:6
              stmt s2 source_item i2 @ 11:5-11:27
              stmt s3 source_item i3 @ 12:5-12:19
              stmt s4 source_item i4 @ 13:5-13:37
              stmt s5 source_item i5 @ 14:5-14:45
              stmt s6 source_item i6 @ 15:5-17:6
              stmt s7 source_item i10 @ 19:5-25:6
              stmt s8 let v0: Alias @ 27:5-27:33
                initializer
                  expr e1 call => nominal struct body_more_local_items_fixture[lib]::crate::GlobalId @ 27:24-27:32
                    callee
                      expr e0 path helper -> fn fn body_more_local_items_fixture[lib]::crate::use_it::helper => <unknown> @ 27:24-27:30
              stmt s9 let v1 @ 28:5-28:27
                initializer
                  expr e2 path DEFAULT -> const fn body_more_local_items_fixture[lib]::crate::use_it::DEFAULT => nominal struct body_more_local_items_fixture[lib]::crate::GlobalId @ 28:19-28:26
              stmt s10 let v2 @ 29:5-29:27
                initializer
                  expr e3 path CURRENT -> static fn body_more_local_items_fixture[lib]::crate::use_it::CURRENT => nominal struct body_more_local_items_fixture[lib]::crate::GlobalId @ 29:19-29:26
              stmt s11 let v3 @ 30:5-30:42
                initializer
                  expr e6 call => nominal enum fn body_more_local_items_fixture[lib]::crate::use_it::Action @ 4:5-7:6 @ 30:18-30:41
                    callee
                      expr e4 path Action::Start -> variant enum fn body_more_local_items_fixture[lib]::crate::use_it::Action @ 4:5-7:6::Start => nominal enum fn body_more_local_items_fixture[lib]::crate::use_it::Action @ 4:5-7:6 @ 30:18-30:31
                    arg
                      expr e5 path GlobalId -> item struct body_more_local_items_fixture[lib]::crate::GlobalId => nominal struct body_more_local_items_fixture[lib]::crate::GlobalId @ 30:32-30:40
        "#]],
    );
}

#[test]
fn resolves_body_local_values_by_scope_before_category() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_local_value_shadowing_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Outer;
pub struct Inner;

pub fn use_it() {
    fn helper() -> Outer {
        Outer
    }
    let value = Outer;

    {
        fn value() -> Inner {
            Inner
        }
        let from_fn = value();
    };

    {
        const helper: Inner = Inner;
        let from_const = helper;
    };
}
"#,
        expect![[r#"
            package body_local_value_shadowing_fixture

            body_local_value_shadowing_fixture [lib]
            body b0 fn body_local_value_shadowing_fixture[lib]::crate::use_it @ 4:1-21:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0; source_items i0
            - s2 parent s1: v1; source_items i1
            - s3 parent s1: v2; source_items i2
            source_items
            - i0 fn helper @ 5:5-7:6
            - i1 fn value @ 11:9-13:10
            - i2 const helper @ 18:9-18:37
            bindings
            - v0 let value `value` => nominal struct body_local_value_shadowing_fixture[lib]::crate::Outer @ 8:9-8:14
            - v1 let from_fn `from_fn` => nominal struct body_local_value_shadowing_fixture[lib]::crate::Inner @ 14:13-14:20
            - v2 let from_const `from_const` => nominal struct body_local_value_shadowing_fixture[lib]::crate::Inner @ 19:13-19:23
            body
            expr e6 block s1 => () @ 4:17-21:2
              stmt s0 source_item i0 @ 5:5-7:6
              stmt s1 let v0 @ 8:5-8:23
                initializer
                  expr e0 path Outer -> item struct body_local_value_shadowing_fixture[lib]::crate::Outer => nominal struct body_local_value_shadowing_fixture[lib]::crate::Outer @ 8:17-8:22
              stmt s4 expr; @ 10:5-15:7
                expr e3 block s2 => () @ 10:5-15:6
                  stmt s2 source_item i1 @ 11:9-13:10
                  stmt s3 let v1 @ 14:9-14:31
                    initializer
                      expr e2 call => nominal struct body_local_value_shadowing_fixture[lib]::crate::Inner @ 14:23-14:30
                        callee
                          expr e1 path value -> fn fn body_local_value_shadowing_fixture[lib]::crate::use_it::value => <unknown> @ 14:23-14:28
              stmt s7 expr; @ 17:5-20:7
                expr e5 block s3 => () @ 17:5-20:6
                  stmt s5 source_item i2 @ 18:9-18:37
                  stmt s6 let v2 @ 19:9-19:33
                    initializer
                      expr e4 path helper -> const fn body_local_value_shadowing_fixture[lib]::crate::use_it::helper => nominal struct body_local_value_shadowing_fixture[lib]::crate::Inner @ 19:26-19:32
        "#]],
    );
}

#[test]
fn resolves_body_local_associated_consts_and_types() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_local_assoc_items_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User;

    impl User {
        const DEFAULT: GlobalId = GlobalId;
        type Id = GlobalId;
    }

    let default = User::DEFAULT;
    let typed: User::Id = GlobalId;
}
"#,
        expect![[r#"
            package body_local_assoc_items_fixture

            body_local_assoc_items_fixture [lib]
            body b0 fn body_local_assoc_items_fixture[lib]::crate::use_it @ 3:1-13:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1; source_items i0, i3
            source_items
            - i0 struct User @ 4:5-4:17
            - i1 const DEFAULT @ 7:9-7:44
            - i2 type_alias Id @ 8:9-8:28
            - i3 impl <unnamed> @ 6:5-9:6
            bindings
            - v0 let default `default` => nominal struct body_local_assoc_items_fixture[lib]::crate::GlobalId @ 11:9-11:16
            - v1 let typed `typed`: User::Id => nominal struct body_local_assoc_items_fixture[lib]::crate::GlobalId @ 12:9-12:14
            body
            expr e2 block s1 => () @ 3:17-13:2
              stmt s0 source_item i0 @ 4:5-4:17
              stmt s1 source_item i3 @ 6:5-9:6
              stmt s2 let v0 @ 11:5-11:33
                initializer
                  expr e0 path User::DEFAULT -> const impl User::DEFAULT => nominal struct body_local_assoc_items_fixture[lib]::crate::GlobalId @ 11:19-11:32
              stmt s3 let v1: User::Id @ 12:5-12:36
                initializer
                  expr e1 path GlobalId -> item struct body_local_assoc_items_fixture[lib]::crate::GlobalId => nominal struct body_local_assoc_items_fixture[lib]::crate::GlobalId @ 12:27-12:35
        "#]],
    );
}

#[test]
fn propagates_body_local_enum_pattern_payload_types() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_local_enum_pattern_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct GlobalId;

pub fn use_it() {
    enum Action {
        Start(User),
        Named { id: GlobalId },
    }

    let action: Action = Action::Start(User);
    let Action::Start(user) = action;
    let named: Action = Action::Start(User);
    let Action::Named { id } = named;
}
"#,
        expect![[r#"
            package body_local_enum_pattern_fixture

            body_local_enum_pattern_fixture [lib]
            body b0 fn body_local_enum_pattern_fixture[lib]::crate::use_it @ 4:1-14:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1, v2, v3; source_items i0
            source_items
            - i0 enum Action @ 5:5-8:6
            bindings
            - v0 let action `action`: Action => nominal enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6 @ 10:9-10:15
            - v1 let user `user` => nominal struct body_local_enum_pattern_fixture[lib]::crate::User @ 11:23-11:27
            - v2 let named `named`: Action => nominal enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6 @ 12:9-12:14
            - v3 let id `id` => nominal struct body_local_enum_pattern_fixture[lib]::crate::GlobalId @ 13:25-13:27
            body
            expr e8 block s1 => () @ 4:17-14:2
              stmt s0 source_item i0 @ 5:5-8:6
              stmt s1 let v0: Action @ 10:5-10:46
                initializer
                  expr e2 call => nominal enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6 @ 10:26-10:45
                    callee
                      expr e0 path Action::Start -> variant enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6::Start => nominal enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6 @ 10:26-10:39
                    arg
                      expr e1 path User -> item struct body_local_enum_pattern_fixture[lib]::crate::User => nominal struct body_local_enum_pattern_fixture[lib]::crate::User @ 10:40-10:44
              stmt s2 let v1 @ 11:5-11:38
                initializer
                  expr e3 path action -> local v0 => nominal enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6 @ 11:31-11:37
              stmt s3 let v2: Action @ 12:5-12:45
                initializer
                  expr e6 call => nominal enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6 @ 12:25-12:44
                    callee
                      expr e4 path Action::Start -> variant enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6::Start => nominal enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6 @ 12:25-12:38
                    arg
                      expr e5 path User -> item struct body_local_enum_pattern_fixture[lib]::crate::User => nominal struct body_local_enum_pattern_fixture[lib]::crate::User @ 12:39-12:43
              stmt s4 let v3 @ 13:5-13:38
                initializer
                  expr e7 path named -> local v2 => nominal enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6 @ 13:32-13:37
        "#]],
    );
}
