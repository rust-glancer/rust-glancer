use expect_test::expect;

use super::utils::{check_project_body_ir, check_project_body_ir_patterns};

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
            - s2 parent s1: <none>
            - s3 parent s1: v2
            - s4 parent s1: <none>
            bindings
            - v0 param maybe `maybe`: Option<User> => nominal enum body_enum_pattern_fixture[lib]::crate::Option<nominal struct body_enum_pattern_fixture[lib]::crate::User> @ 8:15-8:20
            - v1 let value `value` => nominal struct body_enum_pattern_fixture[lib]::crate::User @ 9:14-9:19
            - v2 let user `user` => nominal struct body_enum_pattern_fixture[lib]::crate::User @ 11:14-11:18
            body
            expr e7 block s1 => nominal struct body_enum_pattern_fixture[lib]::crate::User @ 8:36-14:2
              stmt s1 let v1 @ 9:5-9:46
                initializer
                  expr e0 path maybe -> local v0 => nominal enum body_enum_pattern_fixture[lib]::crate::Option<nominal struct body_enum_pattern_fixture[lib]::crate::User> @ 9:23-9:28
                else
                  expr e2 block s2 => () @ 9:34-9:45
                    stmt s0 expr; @ 9:36-9:43
                      expr e1 wrapper return => <unknown> @ 9:36-9:42
              tail
                expr e6 match => nominal struct body_enum_pattern_fixture[lib]::crate::User @ 10:5-13:6
                  scrutinee
                    expr e3 path maybe -> local v0 => nominal enum body_enum_pattern_fixture[lib]::crate::Option<nominal struct body_enum_pattern_fixture[lib]::crate::User> @ 10:11-10:16
                  arm s3
                    expr e4 path user -> local v2 => nominal struct body_enum_pattern_fixture[lib]::crate::User @ 11:23-11:27
                  arm s4
                    expr e5 path value -> local v1 => nominal struct body_enum_pattern_fixture[lib]::crate::User @ 12:17-12:22
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
            - v2 param pair `pair`: (UserId, UserId) => (nominal struct body_destructure_fixture[lib]::crate::UserId, nominal struct body_destructure_fixture[lib]::crate::UserId) @ 10:5-10:9
            - v3 param record `record`: Pair => nominal struct body_destructure_fixture[lib]::crate::Pair @ 11:5-11:11
            - v4 param borrowed `borrowed`: &(UserId, UserId) => &(nominal struct body_destructure_fixture[lib]::crate::UserId, nominal struct body_destructure_fixture[lib]::crate::UserId) @ 12:5-12:13
            - v5 let from_param `from_param`: UserId => nominal struct body_destructure_fixture[lib]::crate::UserId @ 14:9-14:19
            - v6 let left `left` => nominal struct body_destructure_fixture[lib]::crate::UserId @ 15:10-15:14
            - v7 let right `right` => nominal struct body_destructure_fixture[lib]::crate::UserId @ 15:16-15:21
            - v8 let field_left `field_left` => <unknown> @ 16:22-16:32
            - v9 let right `right` => <unknown> @ 16:34-16:39
            - v10 let borrowed_left `borrowed_left` => <unknown> @ 17:11-17:24
            - v11 let borrowed_right `borrowed_right` => <unknown> @ 17:26-17:40
            body
            expr e5 block s1 => nominal struct body_destructure_fixture[lib]::crate::UserId @ 13:13-19:2
              stmt s0 let v5: UserId @ 14:5-14:41
                initializer
                  expr e0 path param_left -> local v0 => <unknown> @ 14:30-14:40
              stmt s1 let v6, v7 @ 15:5-15:30
                initializer
                  expr e1 path pair -> local v2 => (nominal struct body_destructure_fixture[lib]::crate::UserId, nominal struct body_destructure_fixture[lib]::crate::UserId) @ 15:25-15:29
              stmt s2 let v8, v9 @ 16:5-16:51
                initializer
                  expr e2 path record -> local v3 => nominal struct body_destructure_fixture[lib]::crate::Pair @ 16:44-16:50
              stmt s3 let v10, v11 @ 17:5-17:53
                initializer
                  expr e3 path borrowed -> local v4 => &(nominal struct body_destructure_fixture[lib]::crate::UserId, nominal struct body_destructure_fixture[lib]::crate::UserId) @ 17:44-17:52
              tail
                expr e4 path left -> local v6 => nominal struct body_destructure_fixture[lib]::crate::UserId @ 18:5-18:9
        "#]],
    );
}

#[test]
fn resolves_ambiguous_identifier_patterns_without_capitalization_heuristics() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_pattern_resolution_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub const ready: u8 = 1;

pub struct User {
    pub ID: u8,
}

pub fn use_it(value: u8, user: User) -> u8 {
    match value {
        ready => 0,
        other => other,
    }

    let User { ID } = user;
    ID
}

pub fn shadowed(ready: u8, value: u8) -> u8 {
    match value {
        ready => ready,
        _ => 0,
    }
}
"#,
        expect![[r#"
            package body_pattern_resolution_fixture

            body_pattern_resolution_fixture [lib]
            body b0 fn body_pattern_resolution_fixture[lib]::crate::use_it @ 7:1-15:2
            scopes
            - s0 parent <none>: v0, v1
            - s1 parent s0: v3
            - s2 parent s1: <none>
            - s3 parent s1: v2
            bindings
            - v0 param value `value`: u8 => u8 @ 7:15-7:20
            - v1 param user `user`: User => nominal struct body_pattern_resolution_fixture[lib]::crate::User @ 7:26-7:30
            - v2 let other `other` => u8 @ 10:9-10:14
            - v3 let ID `ID` => <unknown> @ 13:16-13:18
            body
            expr e6 block s1 => <unknown> @ 7:44-15:2
              stmt s0 expr @ 8:5-11:6
                expr e3 match => <unknown> @ 8:5-11:6
                  scrutinee
                    expr e0 path value -> local v0 => u8 @ 8:11-8:16
                  arm s2
                    expr e1 literal int `0` => i32 @ 9:18-9:19
                  arm s3
                    expr e2 path other -> local v2 => u8 @ 10:18-10:23
              stmt s1 let v3 @ 13:5-13:28
                initializer
                  expr e4 path user -> local v1 => nominal struct body_pattern_resolution_fixture[lib]::crate::User @ 13:23-13:27
              tail
                expr e5 path ID -> local v3 => <unknown> @ 14:5-14:7


            body b1 fn body_pattern_resolution_fixture[lib]::crate::shadowed @ 17:1-22:2
            scopes
            - s0 parent <none>: v0, v1
            - s1 parent s0: <none>
            - s2 parent s1: v2
            - s3 parent s1: <none>
            bindings
            - v0 param ready `ready`: u8 => u8 @ 17:17-17:22
            - v1 param value `value`: u8 => u8 @ 17:28-17:33
            - v2 let ready `ready` => u8 @ 19:9-19:14
            body
            expr e4 block s1 => <unknown> @ 17:45-22:2
              tail
                expr e3 match => <unknown> @ 18:5-21:6
                  scrutinee
                    expr e0 path value -> local v1 => u8 @ 18:11-18:16
                  arm s2
                    expr e1 path ready -> local v2 => u8 @ 19:18-19:23
                  arm s3
                    expr e2 literal int `0` => i32 @ 20:14-20:15


            body b2 const body_pattern_resolution_fixture[lib]::crate::ready @ 1:1-1:25
            scopes
            - s0 parent <none>: <none>
            bindings
            body
            expr e0 literal int `1` => i32 @ 1:23-1:24
        "#]],
    );
}

#[test]
fn resolves_lowercase_unit_variant_patterns_from_expected_type() {
    check_project_body_ir_patterns(
        r#"
//- /Cargo.toml
[package]
name = "body_lowercase_unit_variant_pattern"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum State {
    idle,
    busy,
}

pub fn use_it(state: State) {
    match state {
        idle => {}
        busy => {}
    }
}
"#,
        expect![[r#"
            package body_lowercase_unit_variant_pattern

            body_lowercase_unit_variant_pattern [lib]
            body b0 fn body_lowercase_unit_variant_pattern[lib]::crate::use_it @ 6:1-11:2
            patterns
            - p0 binding move v0 path state `state` @ 6:15-6:20
            - p1 binding move <none> path idle `idle` @ 8:9-8:13
            - p2 binding move <none> path busy `busy` @ 9:9-9:13
        "#]],
    );
}

#[test]
fn preserves_pattern_modes_rest_and_ambiguity() {
    check_project_body_ir_patterns(
        r#"
//- /Cargo.toml
[package]
name = "body_pattern_shape_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Pair {
    pub left: i32,
    pub right: i32,
}

pub struct TuplePair(pub i32, pub i32, pub i32);

pub enum Maybe {
    None,
}

pub fn use_it(
    tuple: (i32, i32, i32),
    tuple_pair: TuplePair,
    slice: [i32; 3],
    record: Pair,
    borrowed: &mut i32,
    maybe: Maybe,
) {
    let (mut moved, ref shared, ref mut unique) = tuple;
    let &mut ref_target = borrowed;
    let TuplePair(first, .., last) = tuple_pair;
    let [start, .., end] = slice;
    let Pair { left: field_left, right, .. } = record;
    match maybe {
        None => {}
        value => {}
    }
}
"#,
        expect![[r#"
            package body_pattern_shape_fixture

            body_pattern_shape_fixture [lib]
            body b0 fn body_pattern_shape_fixture[lib]::crate::use_it @ 12:1-29:2
            patterns
            - p0 binding move v0 path tuple `tuple` @ 13:5-13:10
            - p1 binding move v1 path tuple_pair `tuple_pair` @ 14:5-14:15
            - p2 binding move v2 path slice `slice` @ 15:5-15:10
            - p3 binding move v3 path record `record` @ 16:5-16:11
            - p4 binding move v4 path borrowed `borrowed` @ 17:5-17:13
            - p5 binding move v5 path maybe `maybe` @ 18:5-18:10
            - p6 binding move mut v6 `mut moved` @ 20:10-20:19
            - p7 binding ref v7 `ref shared` @ 20:21-20:31
            - p8 binding ref mut v8 `ref mut unique` @ 20:33-20:47
            - p9 tuple [p6, p7, p8] `(mut moved, ref shared, ref mut unique)` @ 20:9-20:48
            - p10 binding move v9 path ref_target `ref_target` @ 21:14-21:24
            - p11 ref mut p10 `&mut ref_target` @ 21:9-21:24
            - p12 binding move v10 path first `first` @ 22:19-22:24
            - p13 rest `..` @ 22:26-22:28
            - p14 binding move v11 path last `last` @ 22:30-22:34
            - p15 tuple_struct TuplePair [p12, p13, p14] `TuplePair(first, .., last)` @ 22:9-22:35
            - p16 binding move v12 path start `start` @ 23:10-23:15
            - p17 rest `..` @ 23:17-23:19
            - p18 binding move v13 path end `end` @ 23:21-23:24
            - p19 slice [p16, p17, p18] `[start, .., end]` @ 23:9-23:25
            - p20 binding move v14 path field_left `field_left` @ 24:22-24:32
            - p21 binding move v15 path right `right` @ 24:34-24:39
            - p22 rest `..` @ 24:41-24:43
            - p23 record Pair [left=p20, shorthand right=p21] rest p22 `Pair { left: field_left, right, .. }` @ 24:9-24:45
            - p24 binding move <none> path None `None` @ 26:9-26:13
            - p25 binding move v16 path value `value` @ 27:9-27:14
        "#]],
    );
}

#[test]
fn preserves_record_shorthand_pattern_modes_and_explicit_subpatterns() {
    check_project_body_ir_patterns(
        r#"
//- /Cargo.toml
[package]
name = "body_record_shorthand_pattern_shape"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Option<T> {
    Some(T),
    None,
}

pub struct User {
    pub name: Option<u8>,
}

pub fn use_it(by_ref: User, by_mut: User, by_at: User) {
    let User { ref name } = by_ref;
    let User { mut name } = by_mut;
    match by_at {
        User { name: alias @ Some(_) } => alias,
        User { name: None } => None,
    };
}
"#,
        expect![[r#"
            package body_record_shorthand_pattern_shape

            body_record_shorthand_pattern_shape [lib]
            body b0 fn body_record_shorthand_pattern_shape[lib]::crate::use_it @ 10:1-17:2
            patterns
            - p0 binding move v0 path by_ref `by_ref` @ 10:15-10:21
            - p1 binding move v1 path by_mut `by_mut` @ 10:29-10:35
            - p2 binding move v2 path by_at `by_at` @ 10:43-10:48
            - p3 binding ref v3 `ref name` @ 11:16-11:24
            - p4 record User [shorthand name=p3] `User { ref name }` @ 11:9-11:26
            - p5 binding move mut v4 `mut name` @ 12:16-12:24
            - p6 record User [shorthand name=p5] `User { mut name }` @ 12:9-12:26
            - p7 wildcard `_` @ 14:35-14:36
            - p8 tuple_struct Some [p7] `Some(_)` @ 14:30-14:37
            - p9 binding move v5 subpat p8 `alias @ Some(_)` @ 14:22-14:37
            - p10 record User [name=p9] `User { name: alias @ Some(_) }` @ 14:9-14:39
            - p11 binding move <none> path None `None` @ 15:22-15:26
            - p12 record User [name=p11] `User { name: None }` @ 15:9-15:28
        "#]],
    );
}

#[test]
fn preserves_literal_range_and_const_block_patterns() {
    check_project_body_ir_patterns(
        r#"
//- /Cargo.toml
[package]
name = "body_literal_pattern_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub const min_value: i32 = 0;
pub const max_value: i32 = 10;

pub fn use_it(value: i32) {
    match value {
        -1 | 0..=10 | const { 42 } => {}
        min_value..=max_value => {}
        _ => {}
    }
}
"#,
        expect![[r#"
            package body_literal_pattern_fixture

            body_literal_pattern_fixture [lib]
            body b0 fn body_literal_pattern_fixture[lib]::crate::use_it @ 4:1-10:2
            patterns
            - p0 binding move v0 path value `value` @ 4:15-4:20
            - p1 literal -int `-1` @ 6:9-6:11
            - p2 literal int `0` @ 6:14-6:15
            - p3 literal int `10` @ 6:18-6:20
            - p4 range p2 ..= p3 `0..=10` @ 6:14-6:20
            - p5 const_block e2 `const { 42 }` @ 6:23-6:35
            - p6 or [p1, p4, p5] `-1 | 0..=10 | const { 42 }` @ 6:9-6:35
            - p7 binding move <none> path min_value `min_value` @ 7:9-7:18
            - p8 binding move <none> path max_value `max_value` @ 7:21-7:30
            - p9 range p7 ..= p8 `min_value..=max_value` @ 7:9-7:30
            - p10 wildcard `_` @ 8:9-8:10


            body b1 const body_literal_pattern_fixture[lib]::crate::min_value @ 1:1-1:30
            patterns
            <none>


            body b2 const body_literal_pattern_fixture[lib]::crate::max_value @ 2:1-2:31
            patterns
            <none>
        "#]],
    );
}

#[test]
fn reuses_duplicate_or_pattern_bindings() {
    check_project_body_ir_patterns(
        r#"
//- /Cargo.toml
[package]
name = "body_or_pattern_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Result {
    Ok(i32),
    Err(i32),
}

pub fn use_it(result: Result) {
    match result {
        Result::Ok(value) | Result::Err(value) => {}
    }
}
"#,
        expect![[r#"
            package body_or_pattern_fixture

            body_or_pattern_fixture [lib]
            body b0 fn body_or_pattern_fixture[lib]::crate::use_it @ 6:1-10:2
            patterns
            - p0 binding move v0 path result `result` @ 6:15-6:21
            - p1 binding move v1 path value `value` @ 8:20-8:25
            - p2 tuple_struct Result::Ok [p1] `Result::Ok(value)` @ 8:9-8:26
            - p3 binding move v1 path value `value` @ 8:41-8:46
            - p4 tuple_struct Result::Err [p3] `Result::Err(value)` @ 8:29-8:47
            - p5 or [p2, p4] `Result::Ok(value) | Result::Err(value)` @ 8:9-8:47
        "#]],
    );
}
