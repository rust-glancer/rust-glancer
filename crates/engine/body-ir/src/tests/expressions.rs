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
            - v0 self_param self `&self` => &Self struct body_expr_fixture[lib]::crate::User @ 12:15-12:20
            - v1 param id `id`: UserId => nominal struct body_expr_fixture[lib]::crate::UserId @ 12:22-12:24
            - v2 let this `this`: Self => Self struct body_expr_fixture[lib]::crate::User @ 13:13-13:17
            - v3 let built `built`: UserId => nominal struct body_expr_fixture[lib]::crate::UserId @ 14:13-14:18
            - v4 let via_fn `via_fn`: UserId => nominal struct body_expr_fixture[lib]::crate::UserId @ 15:13-15:19
            - v5 let field `field` => nominal struct body_expr_fixture[lib]::crate::UserId @ 16:13-16:18
            body
            expr e12 block s1 => nominal struct body_expr_fixture[lib]::crate::UserId @ 12:44-18:6
              stmt s0 let v2: Self @ 13:9-13:31
                initializer
                  expr e0 path self -> local v0 => &Self struct body_expr_fixture[lib]::crate::User @ 13:26-13:30
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
                      expr e7 path self -> local v0 => &Self struct body_expr_fixture[lib]::crate::User @ 16:21-16:25
              tail
                expr e11 method_call touch -> fn impl User::touch => nominal struct body_expr_fixture[lib]::crate::UserId @ 17:9-17:27
                  receiver
                    expr e9 path self -> local v0 => &Self struct body_expr_fixture[lib]::crate::User @ 17:9-17:13
                  arg
                    expr e10 path via_fn -> local v4 => nominal struct body_expr_fixture[lib]::crate::UserId @ 17:20-17:26


            body b2 fn impl User::touch @ 20:5-22:6
            scopes
            - s0 parent <none>: v0, v1
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `&self` => &Self struct body_expr_fixture[lib]::crate::User @ 20:14-20:19
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
    let defaults = User { id: 2, .. };
    User { id: 1, ..base }
}
"#,
        expect![[r#"
            package body_record_expr_fixture

            body_record_expr_fixture [lib]
            body b0 fn body_record_expr_fixture[lib]::crate::use_it @ 6:1-10:2
            scopes
            - s0 parent <none>: v0, v1
            - s1 parent s0: v2, v3
            bindings
            - v0 param id `id`: u8 => u8 @ 6:15-6:17
            - v1 param name `name`: u8 => u8 @ 6:23-6:27
            - v2 let base `base` => nominal struct body_record_expr_fixture[lib]::crate::User @ 7:9-7:13
            - v3 let defaults `defaults` => nominal struct body_record_expr_fixture[lib]::crate::User @ 8:9-8:17
            body
            expr e8 block s1 => nominal struct body_record_expr_fixture[lib]::crate::User @ 6:41-10:2
              stmt s0 let v2 @ 7:5-7:34
                initializer
                  expr e2 record User -> item struct body_record_expr_fixture[lib]::crate::User => nominal struct body_record_expr_fixture[lib]::crate::User @ 7:16-7:33
                    field id
                      expr e0 path id -> local v0 => u8 @ 7:23-7:25
                    field name
                      expr e1 path name -> local v1 => u8 @ 7:27-7:31
              stmt s1 let v3 @ 8:5-8:39
                initializer
                  expr e4 record User -> item struct body_record_expr_fixture[lib]::crate::User => nominal struct body_record_expr_fixture[lib]::crate::User @ 8:20-8:38
                    field id
                      expr e3 literal int `2` => <unknown> @ 8:31-8:32
                    spread @ 8:34-8:36
              tail
                expr e7 record User -> item struct body_record_expr_fixture[lib]::crate::User => nominal struct body_record_expr_fixture[lib]::crate::User @ 9:5-9:27
                  field id
                    expr e5 literal int `1` => <unknown> @ 9:16-9:17
                  spread @ 9:19-9:25
                    expr e6 path base -> local v2 => nominal struct body_record_expr_fixture[lib]::crate::User @ 9:21-9:25
        "#]],
    );
}

#[test]
fn preserves_reference_expression_mutability() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_ref_mutability_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it(mut value: u8) {
    let shared = &value;
    let unique = &mut value;
}
"#,
        expect![[r#"
            package body_ref_mutability_fixture

            body_ref_mutability_fixture [lib]
            body b0 fn body_ref_mutability_fixture[lib]::crate::use_it @ 1:1-4:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1, v2
            bindings
            - v0 param value `mut value`: u8 => u8 @ 1:15-1:24
            - v1 let shared `shared` => &u8 @ 2:9-2:15
            - v2 let unique `unique` => &mut u8 @ 3:9-3:15
            body
            expr e4 block s1 => () @ 1:30-4:2
              stmt s0 let v1 @ 2:5-2:25
                initializer
                  expr e1 wrapper ref => &u8 @ 2:18-2:24
                    inner
                      expr e0 path value -> local v0 => u8 @ 2:19-2:24
              stmt s1 let v2 @ 3:5-3:29
                initializer
                  expr e3 wrapper ref => &mut u8 @ 3:18-3:28
                    inner
                      expr e2 path value -> local v0 => u8 @ 3:23-3:28
        "#]],
    );
}

#[test]
fn preserves_rich_body_paths() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_rich_path_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Maybe<T> {
    Some(T),
    None,
}

pub struct User;

pub trait Factory<T> {
    fn make() -> T;
}

impl Factory<User> for User {
    fn make() -> User {
        User
    }
}

pub fn use_it(user: User) {
    let variant = Maybe::<User>::Some(user);
    let qualified = <User as Factory<User>>::make();
}
"#,
        expect![[r#"
            package body_rich_path_fixture

            body_rich_path_fixture [lib]
            body b0 fn body_rich_path_fixture[lib]::crate::use_it @ 18:1-21:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v1, v2
            bindings
            - v0 param user `user`: User => nominal struct body_rich_path_fixture[lib]::crate::User @ 18:15-18:19
            - v1 let variant `variant` => nominal enum body_rich_path_fixture[lib]::crate::Maybe @ 19:9-19:16
            - v2 let qualified `qualified` => <unknown> @ 20:9-20:18
            body
            expr e5 block s1 => () @ 18:27-21:2
              stmt s0 let v1 @ 19:5-19:45
                initializer
                  expr e2 call => nominal enum body_rich_path_fixture[lib]::crate::Maybe @ 19:19-19:44
                    callee
                      expr e0 path Maybe::<User>::Some -> variant enum body_rich_path_fixture[lib]::crate::Maybe::Some => nominal enum body_rich_path_fixture[lib]::crate::Maybe @ 19:19-19:38
                    arg
                      expr e1 path user -> local v0 => nominal struct body_rich_path_fixture[lib]::crate::User @ 19:39-19:43
              stmt s1 let v2 @ 20:5-20:53
                initializer
                  expr e4 call => <unknown> @ 20:21-20:52
                    callee
                      expr e3 path <User as Factory<User>>::make => <unknown> @ 20:21-20:50


            body b1 fn impl Factory<User> for User::make @ 13:5-15:6
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e1 block s1 => nominal struct body_rich_path_fixture[lib]::crate::User @ 13:23-15:6
              tail
                expr e0 path User -> item struct body_rich_path_fixture[lib]::crate::User => nominal struct body_rich_path_fixture[lib]::crate::User @ 14:9-14:13
        "#]],
    );
}

#[test]
fn lowers_common_expression_forms() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_common_expr_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User {
    pub id: u8,
}

pub fn never() -> ! {
    loop {}
}

pub fn use_it(mut pair: (u8, u8), mut slots: [u8; 3], value: u8, user: User) {
    let tuple = (value, user.id);
    let array = [value, 1, 2];
    let repeat = [value; 3];
    let indexed = slots[0];
    let exclusive = 1..value;
    let inclusive = value..=value;
    let full = ..;
    let casted = user as User;
    let field_after_cast = (user as User).id;
    let unary = (!false, -1, *&value);
    let binary = value + 1 == 2 && false || true;
    (pair.0, pair.1) = (1, 2);
    slots[0] += value;
    let hole = _;
    yield value;
    do yeet value;
    become never();
}
"#,
        expect![[r#"
            package body_common_expr_fixture

            body_common_expr_fixture [lib]
            body b0 fn body_common_expr_fixture[lib]::crate::never @ 5:1-7:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            - s2 parent s1: <none>
            bindings
            body
            expr e2 block s1 => <unknown> @ 5:21-7:2
              tail
                expr e1 loop => <unknown> @ 6:5-6:12
                  body
                    expr e0 block s2 => () @ 6:10-6:12


            body b1 fn body_common_expr_fixture[lib]::crate::use_it @ 9:1-27:2
            scopes
            - s0 parent <none>: v0, v1, v2, v3
            - s1 parent s0: v4, v5, v6, v7, v8, v9, v10, v11, v12, v13, v14, v15
            bindings
            - v0 param pair `mut pair`: (u8, u8) => syntax (u8, u8) @ 9:15-9:23
            - v1 param slots `mut slots`: [u8; 3] => syntax [u8; 3] @ 9:35-9:44
            - v2 param value `value`: u8 => u8 @ 9:55-9:60
            - v3 param user `user`: User => nominal struct body_common_expr_fixture[lib]::crate::User @ 9:66-9:70
            - v4 let tuple `tuple` => <unknown> @ 10:9-10:14
            - v5 let array `array` => <unknown> @ 11:9-11:14
            - v6 let repeat `repeat` => <unknown> @ 12:9-12:15
            - v7 let indexed `indexed` => <unknown> @ 13:9-13:16
            - v8 let exclusive `exclusive` => <unknown> @ 14:9-14:18
            - v9 let inclusive `inclusive` => <unknown> @ 15:9-15:18
            - v10 let full `full` => <unknown> @ 16:9-16:13
            - v11 let casted `casted` => nominal struct body_common_expr_fixture[lib]::crate::User @ 17:9-17:15
            - v12 let field_after_cast `field_after_cast` => u8 @ 18:9-18:25
            - v13 let unary `unary` => <unknown> @ 19:9-19:14
            - v14 let binary `binary` => <unknown> @ 20:9-20:15
            - v15 let hole `hole` => <unknown> @ 23:9-23:13
            body
            expr e66 block s1 => () @ 9:78-27:2
              stmt s0 let v4 @ 10:5-10:34
                initializer
                  expr e3 tuple => <unknown> @ 10:17-10:33
                    field
                      expr e0 path value -> local v2 => u8 @ 10:18-10:23
                    field
                      expr e2 field id -> field struct body_common_expr_fixture[lib]::crate::User::id => u8 @ 10:25-10:32
                        base
                          expr e1 path user -> local v3 => nominal struct body_common_expr_fixture[lib]::crate::User @ 10:25-10:29
              stmt s1 let v5 @ 11:5-11:31
                initializer
                  expr e7 array => <unknown> @ 11:17-11:30
                    element
                      expr e4 path value -> local v2 => u8 @ 11:18-11:23
                    element
                      expr e5 literal int `1` => <unknown> @ 11:25-11:26
                    element
                      expr e6 literal int `2` => <unknown> @ 11:28-11:29
              stmt s2 let v6 @ 12:5-12:29
                initializer
                  expr e10 repeat_array => <unknown> @ 12:18-12:28
                    initializer
                      expr e8 path value -> local v2 => u8 @ 12:19-12:24
                    repeat
                      expr e9 literal int `3` => <unknown> @ 12:26-12:27
              stmt s3 let v7 @ 13:5-13:28
                initializer
                  expr e13 index => <unknown> @ 13:19-13:27
                    base
                      expr e11 path slots -> local v1 => syntax [u8; 3] @ 13:19-13:24
                    index
                      expr e12 literal int `0` => <unknown> @ 13:25-13:26
              stmt s4 let v8 @ 14:5-14:30
                initializer
                  expr e16 range .. => <unknown> @ 14:21-14:29
                    start
                      expr e14 literal int `1` => <unknown> @ 14:21-14:22
                    end
                      expr e15 path value -> local v2 => u8 @ 14:24-14:29
              stmt s5 let v9 @ 15:5-15:35
                initializer
                  expr e19 range ..= => <unknown> @ 15:21-15:34
                    start
                      expr e17 path value -> local v2 => u8 @ 15:21-15:26
                    end
                      expr e18 path value -> local v2 => u8 @ 15:29-15:34
              stmt s6 let v10 @ 16:5-16:19
                initializer
                  expr e20 range .. => <unknown> @ 16:16-16:18
              stmt s7 let v11 @ 17:5-17:31
                initializer
                  expr e22 cast as User => nominal struct body_common_expr_fixture[lib]::crate::User @ 17:18-17:30
                    inner
                      expr e21 path user -> local v3 => nominal struct body_common_expr_fixture[lib]::crate::User @ 17:18-17:22
              stmt s8 let v12 @ 18:5-18:46
                initializer
                  expr e26 field id -> field struct body_common_expr_fixture[lib]::crate::User::id => u8 @ 18:28-18:45
                    base
                      expr e25 wrapper paren => nominal struct body_common_expr_fixture[lib]::crate::User @ 18:28-18:42
                        inner
                          expr e24 cast as User => nominal struct body_common_expr_fixture[lib]::crate::User @ 18:29-18:41
                            inner
                              expr e23 path user -> local v3 => nominal struct body_common_expr_fixture[lib]::crate::User @ 18:29-18:33
              stmt s9 let v13 @ 19:5-19:39
                initializer
                  expr e34 tuple => <unknown> @ 19:17-19:38
                    field
                      expr e28 unary ! => <unknown> @ 19:18-19:24
                        inner
                          expr e27 literal bool `false` => <unknown> @ 19:19-19:24
                    field
                      expr e30 unary - => <unknown> @ 19:26-19:28
                        inner
                          expr e29 literal int `1` => <unknown> @ 19:27-19:28
                    field
                      expr e33 unary * => u8 @ 19:30-19:37
                        inner
                          expr e32 wrapper ref => &u8 @ 19:31-19:37
                            inner
                              expr e31 path value -> local v2 => u8 @ 19:32-19:37
              stmt s10 let v14 @ 20:5-20:50
                initializer
                  expr e43 binary || => <unknown> @ 20:18-20:49
                    lhs
                      expr e41 binary && => <unknown> @ 20:18-20:41
                        lhs
                          expr e39 binary == => <unknown> @ 20:18-20:32
                            lhs
                              expr e37 binary + => <unknown> @ 20:18-20:27
                                lhs
                                  expr e35 path value -> local v2 => u8 @ 20:18-20:23
                                rhs
                                  expr e36 literal int `1` => <unknown> @ 20:26-20:27
                            rhs
                              expr e38 literal int `2` => <unknown> @ 20:31-20:32
                        rhs
                          expr e40 literal bool `false` => <unknown> @ 20:36-20:41
                    rhs
                      expr e42 literal bool `true` => <unknown> @ 20:45-20:49
              stmt s11 expr; @ 21:5-21:31
                expr e52 assign = => () @ 21:5-21:30
                  target
                    expr e48 tuple => <unknown> @ 21:5-21:21
                      field
                        expr e45 field 0 => <unknown> @ 21:6-21:12
                          base
                            expr e44 path pair -> local v0 => syntax (u8, u8) @ 21:6-21:10
                      field
                        expr e47 field 1 => <unknown> @ 21:14-21:20
                          base
                            expr e46 path pair -> local v0 => syntax (u8, u8) @ 21:14-21:18
                  value
                    expr e51 tuple => <unknown> @ 21:24-21:30
                      field
                        expr e49 literal int `1` => <unknown> @ 21:25-21:26
                      field
                        expr e50 literal int `2` => <unknown> @ 21:28-21:29
              stmt s12 expr; @ 22:5-22:23
                expr e57 assign += => () @ 22:5-22:22
                  target
                    expr e55 index => <unknown> @ 22:5-22:13
                      base
                        expr e53 path slots -> local v1 => syntax [u8; 3] @ 22:5-22:10
                      index
                        expr e54 literal int `0` => <unknown> @ 22:11-22:12
                  value
                    expr e56 path value -> local v2 => u8 @ 22:17-22:22
              stmt s13 let v15 @ 23:5-23:18
                initializer
                  expr e58 underscore => <unknown> @ 23:16-23:17
              stmt s14 expr; @ 24:5-24:17
                expr e60 yield => <unknown> @ 24:5-24:16
                  value
                    expr e59 path value -> local v2 => u8 @ 24:11-24:16
              stmt s15 expr; @ 25:5-25:19
                expr e62 yeet => ! @ 25:5-25:18
                  value
                    expr e61 path value -> local v2 => u8 @ 25:13-25:18
              stmt s16 expr; @ 26:5-26:20
                expr e65 become => ! @ 26:5-26:19
                  value
                    expr e64 call => ! @ 26:12-26:19
                      callee
                        expr e63 path never -> item fn body_common_expr_fixture[lib]::crate::never => <unknown> @ 26:12-26:17
        "#]],
    );
}

#[test]
fn lowers_block_modifiers() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_block_modifier_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it(value: u8) {
    unsafe { value; };
    const { value; };
    async { value };
    async move { value };
    try { value };
    try bikeshed Result<u8> { value };
    gen { yield value; };
    gen move { yield value; };
    async gen { yield value; };
    async gen move { yield value; };
}
"#,
        expect![[r#"
            package body_block_modifier_fixture

            body_block_modifier_fixture [lib]
            body b0 fn body_block_modifier_fixture[lib]::crate::use_it @ 1:1-12:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            - s2 parent s1: <none>
            - s3 parent s1: <none>
            - s4 parent s1: <none>
            - s5 parent s1: <none>
            - s6 parent s1: <none>
            - s7 parent s1: <none>
            - s8 parent s1: <none>
            - s9 parent s1: <none>
            - s10 parent s1: <none>
            - s11 parent s1: <none>
            bindings
            - v0 param value `value`: u8 => u8 @ 1:15-1:20
            body
            expr e24 block s1 => () @ 1:26-12:2
              stmt s1 expr; @ 2:5-2:23
                expr e1 block unsafe s2 => () @ 2:5-2:22
                  stmt s0 expr; @ 2:14-2:20
                    expr e0 path value -> local v0 => u8 @ 2:14-2:19
              stmt s3 expr; @ 3:5-3:22
                expr e3 block const s3 => () @ 3:5-3:21
                  stmt s2 expr; @ 3:13-3:19
                    expr e2 path value -> local v0 => u8 @ 3:13-3:18
              stmt s4 expr; @ 4:5-4:21
                expr e5 block async s4 => u8 @ 4:5-4:20
                  tail
                    expr e4 path value -> local v0 => u8 @ 4:13-4:18
              stmt s5 expr; @ 5:5-5:26
                expr e7 block async move s5 => u8 @ 5:5-5:25
                  tail
                    expr e6 path value -> local v0 => u8 @ 5:18-5:23
              stmt s6 expr; @ 6:5-6:19
                expr e9 block try s6 => u8 @ 6:5-6:18
                  tail
                    expr e8 path value -> local v0 => u8 @ 6:11-6:16
              stmt s7 expr; @ 7:5-7:39
                expr e11 block try bikeshed Result<u8> s7 => u8 @ 7:5-7:38
                  tail
                    expr e10 path value -> local v0 => u8 @ 7:31-7:36
              stmt s9 expr; @ 8:5-8:26
                expr e14 block gen s8 => () @ 8:5-8:25
                  stmt s8 expr; @ 8:11-8:23
                    expr e13 yield => <unknown> @ 8:11-8:22
                      value
                        expr e12 path value -> local v0 => u8 @ 8:17-8:22
              stmt s11 expr; @ 9:5-9:31
                expr e17 block gen move s9 => () @ 9:5-9:30
                  stmt s10 expr; @ 9:16-9:28
                    expr e16 yield => <unknown> @ 9:16-9:27
                      value
                        expr e15 path value -> local v0 => u8 @ 9:22-9:27
              stmt s13 expr; @ 10:5-10:32
                expr e20 block async gen s10 => () @ 10:5-10:31
                  stmt s12 expr; @ 10:17-10:29
                    expr e19 yield => <unknown> @ 10:17-10:28
                      value
                        expr e18 path value -> local v0 => u8 @ 10:23-10:28
              stmt s15 expr; @ 11:5-11:37
                expr e23 block async gen move s11 => () @ 11:5-11:36
                  stmt s14 expr; @ 11:22-11:34
                    expr e22 yield => <unknown> @ 11:22-11:33
                      value
                        expr e21 path value -> local v0 => u8 @ 11:28-11:33
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
