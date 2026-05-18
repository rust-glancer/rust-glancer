use expect_test::expect;

use super::utils::check_project_body_ir;

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
fn records_struct_literals_as_record_expressions() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_record_expr_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User {
    pub id: u8,
    pub name: u8,
}

pub fn use_it(id: u8, name: u8) -> User {
    let base = User { id, name };
    User { id: 1, ..base }
}
"#,
        expect![[r#"
            package body_record_expr_fixture

            body_record_expr_fixture [lib]
            body b0 fn body_record_expr_fixture[lib]::crate::use_it @ 6:1-9:2
            scopes
            - s0 parent <none>: v0, v1
            - s1 parent s0: v2
            bindings
            - v0 param id `id`: u8 => syntax u8 @ 6:15-6:17
            - v1 param name `name`: u8 => syntax u8 @ 6:23-6:27
            - v2 let base `base` => nominal struct body_record_expr_fixture[lib]::crate::User @ 7:9-7:13
            body
            expr e6 block s1 => nominal struct body_record_expr_fixture[lib]::crate::User @ 6:41-9:2
              stmt s0 let v2 @ 7:5-7:34
                initializer
                  expr e2 record User -> item struct body_record_expr_fixture[lib]::crate::User => nominal struct body_record_expr_fixture[lib]::crate::User @ 7:16-7:33
                    field id
                      expr e0 path id -> local v0 => syntax u8 @ 7:23-7:25
                    field name
                      expr e1 path name -> local v1 => syntax u8 @ 7:27-7:31
              tail
                expr e5 record User -> item struct body_record_expr_fixture[lib]::crate::User => nominal struct body_record_expr_fixture[lib]::crate::User @ 8:5-8:27
                  field id
                    expr e3 literal int `1` => <unknown> @ 8:16-8:17
                  spread
                    expr e4 path base -> local v2 => nominal struct body_record_expr_fixture[lib]::crate::User @ 8:21-8:25
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
