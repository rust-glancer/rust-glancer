mod utils;

use crate::BodyIrBuildPolicy;
use expect_test::expect;

use self::utils::{check_project_body_ir, check_project_body_ir_with_policy};

#[test]
fn skips_non_workspace_package_bodies_by_default() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_policy_app"
version = "0.1.0"
edition = "2024"

[dependencies]
body_policy_dep = { path = "dep" }

//- /src/lib.rs
pub fn app() {}

//- /dep/Cargo.toml
[package]
name = "body_policy_dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub fn dep() {}
"#,
        expect![[r#"
            package body_policy_app

            body_policy_app [lib]
            body b0 fn body_policy_app[lib]::crate::app @ 1:1-1:16
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e0 block s1 => () @ 1:14-1:16


            package body_policy_dep

            body_policy_dep [lib]
            skipped
        "#]],
    );
}

#[test]
fn can_lower_non_workspace_package_bodies_when_requested() {
    check_project_body_ir_with_policy(
        r#"
//- /Cargo.toml
[package]
name = "body_policy_app"
version = "0.1.0"
edition = "2024"

[dependencies]
body_policy_dep = { path = "dep" }

//- /src/lib.rs
pub fn app() {}

//- /dep/Cargo.toml
[package]
name = "body_policy_dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub fn dep() {}
"#,
        BodyIrBuildPolicy::all_packages(),
        expect![[r#"
            package body_policy_app

            body_policy_app [lib]
            body b0 fn body_policy_app[lib]::crate::app @ 1:1-1:16
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e0 block s1 => () @ 1:14-1:16


            package body_policy_dep

            body_policy_dep [lib]
            body b0 fn body_policy_dep[lib]::crate::dep @ 1:1-1:16
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e0 block s1 => () @ 1:14-1:16
        "#]],
    );
}

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

#[test]
fn records_calls_fields_methods_and_easy_types() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_expr_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct UserId(u64);

pub struct User {
    pub id: UserId,
}

pub fn identity(id: UserId) -> UserId {
    id
}

impl User {
    pub fn id(&self, id: UserId) -> UserId {
        let this: Self = self;
        let built: UserId = UserId(1);
        let via_fn: UserId = identity(id);
        let field = self.id;
        self.touch(via_fn)
    }

    fn touch(&self, id: UserId) -> UserId {
        id
    }
}
"#,
        expect![[r#"
            package body_expr_fixture

            body_expr_fixture [lib]
            body b0 fn body_expr_fixture[lib]::crate::identity @ 7:1-9:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 param id `id`: UserId => nominal struct body_expr_fixture[lib]::crate::UserId @ 7:17-7:19
            body
            expr e1 block s1 => nominal struct body_expr_fixture[lib]::crate::UserId @ 7:39-9:2
              tail
                expr e0 path id -> local v0 => nominal struct body_expr_fixture[lib]::crate::UserId @ 8:5-8:7


            body b1 fn impl User::id @ 12:5-18:6
            scopes
            - s0 parent <none>: v0, v1
            - s1 parent s0: v2, v3, v4, v5
            bindings
            - v0 self_param self `&self` => Self struct body_expr_fixture[lib]::crate::User @ 12:15-12:20
            - v1 param id `id`: UserId => nominal struct body_expr_fixture[lib]::crate::UserId @ 12:22-12:24
            - v2 let this `this`: Self => Self struct body_expr_fixture[lib]::crate::User @ 13:13-13:17
            - v3 let built `built`: UserId => nominal struct body_expr_fixture[lib]::crate::UserId @ 14:13-14:18
            - v4 let via_fn `via_fn`: UserId => nominal struct body_expr_fixture[lib]::crate::UserId @ 15:13-15:19
            - v5 let field `field` => nominal struct body_expr_fixture[lib]::crate::UserId @ 16:13-16:18
            body
            expr e12 block s1 => nominal struct body_expr_fixture[lib]::crate::UserId @ 12:44-18:6
              stmt s0 let v2: Self @ 13:9-13:31
                initializer
                  expr e0 path self -> local v0 => Self struct body_expr_fixture[lib]::crate::User @ 13:26-13:30
              stmt s1 let v3: UserId @ 14:9-14:39
                initializer
                  expr e3 call => nominal struct body_expr_fixture[lib]::crate::UserId @ 14:29-14:38
                    callee
                      expr e1 path UserId -> item struct body_expr_fixture[lib]::crate::UserId => nominal struct body_expr_fixture[lib]::crate::UserId @ 14:29-14:35
                    arg
                      expr e2 literal int `1` => <unknown> @ 14:36-14:37
              stmt s2 let v4: UserId @ 15:9-15:43
                initializer
                  expr e6 call => nominal struct body_expr_fixture[lib]::crate::UserId @ 15:30-15:42
                    callee
                      expr e4 path identity -> item fn body_expr_fixture[lib]::crate::identity => <unknown> @ 15:30-15:38
                    arg
                      expr e5 path id -> local v1 => nominal struct body_expr_fixture[lib]::crate::UserId @ 15:39-15:41
              stmt s3 let v5 @ 16:9-16:29
                initializer
                  expr e8 field id -> field struct body_expr_fixture[lib]::crate::User::id => nominal struct body_expr_fixture[lib]::crate::UserId @ 16:21-16:28
                    base
                      expr e7 path self -> local v0 => Self struct body_expr_fixture[lib]::crate::User @ 16:21-16:25
              tail
                expr e11 method_call touch -> fn impl User::touch => nominal struct body_expr_fixture[lib]::crate::UserId @ 17:9-17:27
                  receiver
                    expr e9 path self -> local v0 => Self struct body_expr_fixture[lib]::crate::User @ 17:9-17:13
                  arg
                    expr e10 path via_fn -> local v4 => nominal struct body_expr_fixture[lib]::crate::UserId @ 17:20-17:26


            body b2 fn impl User::touch @ 20:5-22:6
            scopes
            - s0 parent <none>: v0, v1
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `&self` => Self struct body_expr_fixture[lib]::crate::User @ 20:14-20:19
            - v1 param id `id`: UserId => nominal struct body_expr_fixture[lib]::crate::UserId @ 20:21-20:23
            body
            expr e1 block s1 => nominal struct body_expr_fixture[lib]::crate::UserId @ 20:43-22:6
              tail
                expr e0 path id -> local v1 => nominal struct body_expr_fixture[lib]::crate::UserId @ 21:9-21:11
        "#]],
    );
}

#[test]
fn resolves_associated_functions_and_enum_variant_calls() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_associated_path_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Widget;

impl Widget {
    pub fn create() -> Self {
        Widget
    }
}

pub enum Action {
    Configure(Widget),
}

pub fn use_it() {
    let widget = Widget::create();
    let action = Action::Configure(widget);
}
"#,
        expect![[r#"
            package body_associated_path_fixture

            body_associated_path_fixture [lib]
            body b0 fn body_associated_path_fixture[lib]::crate::use_it @ 13:1-16:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1
            bindings
            - v0 let widget `widget` => Self struct body_associated_path_fixture[lib]::crate::Widget @ 14:9-14:15
            - v1 let action `action` => nominal enum body_associated_path_fixture[lib]::crate::Action @ 15:9-15:15
            body
            expr e5 block s1 => () @ 13:17-16:2
              stmt s0 let v0 @ 14:5-14:35
                initializer
                  expr e1 call => Self struct body_associated_path_fixture[lib]::crate::Widget @ 14:18-14:34
                    callee
                      expr e0 path Widget::create -> fn impl Widget::create => <unknown> @ 14:18-14:32
              stmt s1 let v1 @ 15:5-15:44
                initializer
                  expr e4 call => nominal enum body_associated_path_fixture[lib]::crate::Action @ 15:18-15:43
                    callee
                      expr e2 path Action::Configure -> variant enum body_associated_path_fixture[lib]::crate::Action::Configure => nominal enum body_associated_path_fixture[lib]::crate::Action @ 15:18-15:35
                    arg
                      expr e3 path widget -> local v0 => Self struct body_associated_path_fixture[lib]::crate::Widget @ 15:36-15:42


            body b1 fn impl Widget::create @ 4:5-6:6
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e1 block s1 => nominal struct body_associated_path_fixture[lib]::crate::Widget @ 4:29-6:6
              tail
                expr e0 path Widget -> item struct body_associated_path_fixture[lib]::crate::Widget => nominal struct body_associated_path_fixture[lib]::crate::Widget @ 5:9-5:15
        "#]],
    );
}

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
fn propagates_enum_variant_payload_types_through_patterns() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_enum_pattern_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub enum Option<T> {
    Some(T),
    None,
}

pub fn use_it(maybe: Option<User>) {
    let Some(value) = maybe else { return; };
    match maybe {
        Some(user) => user,
        None => value,
    }
}
"#,
        expect![[r#"
            package body_enum_pattern_fixture

            body_enum_pattern_fixture [lib]
            body b0 fn body_enum_pattern_fixture[lib]::crate::use_it @ 8:1-14:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1
            - s2 parent s1: v2
            - s3 parent s1: <none>
            bindings
            - v0 param maybe `maybe`: Option<User> => nominal enum body_enum_pattern_fixture[lib]::crate::Option<nominal struct body_enum_pattern_fixture[lib]::crate::User> @ 8:15-8:20
            - v1 let value `value` => nominal struct body_enum_pattern_fixture[lib]::crate::User @ 9:14-9:19
            - v2 let user `user` => nominal struct body_enum_pattern_fixture[lib]::crate::User @ 11:14-11:18
            body
            expr e5 block s1 => nominal struct body_enum_pattern_fixture[lib]::crate::User @ 8:36-14:2
              stmt s0 let v1 @ 9:5-9:46
                initializer
                  expr e0 path maybe -> local v0 => nominal enum body_enum_pattern_fixture[lib]::crate::Option<nominal struct body_enum_pattern_fixture[lib]::crate::User> @ 9:23-9:28
              tail
                expr e4 match => nominal struct body_enum_pattern_fixture[lib]::crate::User @ 10:5-13:6
                  scrutinee
                    expr e1 path maybe -> local v0 => nominal enum body_enum_pattern_fixture[lib]::crate::Option<nominal struct body_enum_pattern_fixture[lib]::crate::User> @ 10:11-10:16
                  arm s2
                    expr e2 path user -> local v2 => nominal struct body_enum_pattern_fixture[lib]::crate::User @ 11:23-11:27
                  arm s3
                    expr e3 path value -> local v1 => nominal struct body_enum_pattern_fixture[lib]::crate::User @ 12:17-12:22
        "#]],
    );
}

#[test]
fn collects_bindings_from_destructuring_patterns() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_destructure_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct UserId(u64);

pub struct Pair {
    pub left: UserId,
    pub right: UserId,
}

pub fn destructure(
    (param_left, param_right): (UserId, UserId),
    pair: (UserId, UserId),
    record: Pair,
    borrowed: &(UserId, UserId),
) -> UserId {
    let from_param: UserId = param_left;
    let (left, right) = pair;
    let Pair { left: field_left, right } = record;
    let &(borrowed_left, borrowed_right) = borrowed;
    left
}
"#,
        expect![[r#"
            package body_destructure_fixture

            body_destructure_fixture [lib]
            body b0 fn body_destructure_fixture[lib]::crate::destructure @ 8:1-19:2
            scopes
            - s0 parent <none>: v0, v1, v2, v3, v4
            - s1 parent s0: v5, v6, v7, v8, v9, v10, v11
            bindings
            - v0 param param_left `param_left` => <unknown> @ 9:6-9:16
            - v1 param param_right `param_right` => <unknown> @ 9:18-9:29
            - v2 param pair `pair`: (UserId, UserId) => syntax (UserId, UserId) @ 10:5-10:9
            - v3 param record `record`: Pair => nominal struct body_destructure_fixture[lib]::crate::Pair @ 11:5-11:11
            - v4 param borrowed `borrowed`: &(UserId, UserId) => &syntax (UserId, UserId) @ 12:5-12:13
            - v5 let from_param `from_param`: UserId => nominal struct body_destructure_fixture[lib]::crate::UserId @ 14:9-14:19
            - v6 let left `left` => <unknown> @ 15:10-15:14
            - v7 let right `right` => <unknown> @ 15:16-15:21
            - v8 let field_left `field_left` => <unknown> @ 16:22-16:32
            - v9 let right `right` => <unknown> @ 16:34-16:39
            - v10 let borrowed_left `borrowed_left` => <unknown> @ 17:11-17:24
            - v11 let borrowed_right `borrowed_right` => <unknown> @ 17:26-17:40
            body
            expr e5 block s1 => <unknown> @ 13:13-19:2
              stmt s0 let v5: UserId @ 14:5-14:41
                initializer
                  expr e0 path param_left -> local v0 => <unknown> @ 14:30-14:40
              stmt s1 let v6, v7 @ 15:5-15:30
                initializer
                  expr e1 path pair -> local v2 => syntax (UserId, UserId) @ 15:25-15:29
              stmt s2 let v8, v9 @ 16:5-16:51
                initializer
                  expr e2 path record -> local v3 => nominal struct body_destructure_fixture[lib]::crate::Pair @ 16:44-16:50
              stmt s3 let v10, v11 @ 17:5-17:53
                initializer
                  expr e3 path borrowed -> local v4 => &syntax (UserId, UserId) @ 17:44-17:52
              tail
                expr e4 path left -> local v6 => <unknown> @ 18:5-18:9
        "#]],
    );
}

#[test]
fn resolves_tuple_field_accesses() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_tuple_field_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Left;
pub struct Right;

pub struct Pair(pub Left, pub Right);

pub fn use_it(pair: Pair) -> Right {
    pair.1
}
"#,
        expect![[r#"
            package body_tuple_field_fixture

            body_tuple_field_fixture [lib]
            body b0 fn body_tuple_field_fixture[lib]::crate::use_it @ 6:1-8:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 param pair `pair`: Pair => nominal struct body_tuple_field_fixture[lib]::crate::Pair @ 6:15-6:19
            body
            expr e2 block s1 => nominal struct body_tuple_field_fixture[lib]::crate::Right @ 6:36-8:2
              tail
                expr e1 field 1 -> field struct body_tuple_field_fixture[lib]::crate::Pair::#1 => nominal struct body_tuple_field_fixture[lib]::crate::Right @ 7:5-7:11
                  base
                    expr e0 path pair -> local v0 => nominal struct body_tuple_field_fixture[lib]::crate::Pair @ 7:5-7:9
        "#]],
    );
}

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
            - v0 self_param self `&self` => Self struct body_trait_applicability_fixture[lib]::crate::User @ 13:15-13:20
            body
            expr e1 block s1 => nominal struct body_trait_applicability_fixture[lib]::crate::User @ 13:30-15:6
              tail
                expr e0 path User -> item struct body_trait_applicability_fixture[lib]::crate::User => nominal struct body_trait_applicability_fixture[lib]::crate::User @ 14:9-14:13


            body b2 fn impl GenericTrait for Wrapper<T>::generic @ 23:5-25:6
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `&self` => Self struct body_trait_applicability_fixture[lib]::crate::Wrapper @ 23:16-23:21
            body
            expr e1 block s1 => nominal struct body_trait_applicability_fixture[lib]::crate::User @ 23:31-25:6
              tail
                expr e0 path User -> item struct body_trait_applicability_fixture[lib]::crate::User => nominal struct body_trait_applicability_fixture[lib]::crate::User @ 24:9-24:13


            body b3 fn impl UserOnlyTrait for Wrapper<User>::user_only @ 33:5-35:6
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `&self` => Self struct body_trait_applicability_fixture[lib]::crate::Wrapper @ 33:18-33:23
            body
            expr e1 block s1 => nominal struct body_trait_applicability_fixture[lib]::crate::User @ 33:33-35:6
              tail
                expr e0 path User -> item struct body_trait_applicability_fixture[lib]::crate::User => nominal struct body_trait_applicability_fixture[lib]::crate::User @ 34:9-34:13
        "#]],
    );
}

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
