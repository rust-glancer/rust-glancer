use expect_test::expect;

use super::utils::{check_project_body_ir, check_project_body_ir_with_fake_sysroot};

#[test]
fn lowers_closure_scopes_params_and_body() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_closure_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn use_it(user: User) -> User {
    let pick = async move |user: User| -> User { user };
    let pair = |(left, right): (User, User)| left;
    user
}
"#,
        expect![[r#"
            package body_closure_fixture

            body_closure_fixture [lib]
            body b0 fn body_closure_fixture[lib]::crate::use_it @ 3:1-7:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v2, v5
            - s2 parent s1: v1
            - s3 parent s2: <none>
            - s4 parent s1: v3, v4
            bindings
            - v0 param user `user`: User => nominal struct body_closure_fixture[lib]::crate::User @ 3:15-3:19
            - v1 param user `user`: User => nominal struct body_closure_fixture[lib]::crate::User @ 4:28-4:32
            - v2 let pick `pick` => closure #2 @ 4:9-4:13
            - v3 param left `left` => nominal struct body_closure_fixture[lib]::crate::User @ 5:18-5:22
            - v4 param right `right` => nominal struct body_closure_fixture[lib]::crate::User @ 5:24-5:29
            - v5 let pair `pair` => closure #4 @ 5:9-5:13
            body
            expr e6 block s1 => nominal struct body_closure_fixture[lib]::crate::User @ 3:35-7:2
              stmt s0 let v2 @ 4:5-4:57
                initializer
                  expr e2 closure async move s2 (v1: User) -> User => closure #2 @ 4:16-4:56
                    body
                      expr e1 block s3 => nominal struct body_closure_fixture[lib]::crate::User @ 4:48-4:56
                        tail
                          expr e0 path user -> local v1 => nominal struct body_closure_fixture[lib]::crate::User @ 4:50-4:54
              stmt s1 let v5 @ 5:5-5:51
                initializer
                  expr e4 closure s4 (v3, v4: (User, User)) => closure #4 @ 5:16-5:50
                    body
                      expr e3 path left -> local v3 => nominal struct body_closure_fixture[lib]::crate::User @ 5:46-5:50
              tail
                expr e5 path user -> local v0 => nominal struct body_closure_fixture[lib]::crate::User @ 6:5-6:9
        "#]],
    );
}

#[test]
fn lowers_unannotated_closure_params_as_scope_bindings() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_untyped_closure_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn use_it(user: User) -> User {
    let echo = |user| user;
    user
}
"#,
        expect![[r#"
            package body_untyped_closure_fixture

            body_untyped_closure_fixture [lib]
            body b0 fn body_untyped_closure_fixture[lib]::crate::use_it @ 3:1-6:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: v2
            - s2 parent s1: v1
            bindings
            - v0 param user `user`: User => nominal struct body_untyped_closure_fixture[lib]::crate::User @ 3:15-3:19
            - v1 param user `user` => <unknown> @ 4:17-4:21
            - v2 let echo `echo` => closure #1 @ 4:9-4:13
            body
            expr e3 block s1 => nominal struct body_untyped_closure_fixture[lib]::crate::User @ 3:35-6:2
              stmt s0 let v2 @ 4:5-4:28
                initializer
                  expr e1 closure s2 (v1) => closure #1 @ 4:16-4:27
                    body
                      expr e0 path user -> local v1 => <unknown> @ 4:23-4:27
              tail
                expr e2 path user -> local v0 => nominal struct body_untyped_closure_fixture[lib]::crate::User @ 5:5-5:9
        "#]],
    );
}

#[test]
fn infers_closure_params_from_direct_fn_trait_expectations() {
    check_project_body_ir_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "body_direct_closure_expectation_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Attr;

pub struct AttrVec;

impl AttrVec {
    pub fn push(&mut self, attr: Attr) {}
}

pub struct User;

pub fn with_attrs(f: impl FnOnce(&mut AttrVec)) {}
pub fn with_pair(f: impl FnOnce((User, User)) -> User) {}

pub fn use_it(attr: Attr) {
    with_attrs(|attrs| attrs.push(attr));
    with_pair(|(left, right)| left);
}
"#,
        expect![[r#"
            package alloc

            alloc [lib]
            skipped

            package body_direct_closure_expectation_fixture

            body_direct_closure_expectation_fixture [lib]
            body b0 fn body_direct_closure_expectation_fixture[lib]::crate::with_attrs @ 11:1-11:51
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 param f `f`: impl FnOnce(&mut AttrVec) => impl trait core[lib]::crate::FnOnce<(&mut nominal struct body_direct_closure_expectation_fixture[lib]::crate::AttrVec,), Output = ()> @ 11:19-11:20
            body
            expr e0 block s1 => () @ 11:49-11:51


            body b1 fn body_direct_closure_expectation_fixture[lib]::crate::with_pair @ 12:1-12:58
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 param f `f`: impl FnOnce((User, User)) -> User => impl trait core[lib]::crate::FnOnce<((nominal struct body_direct_closure_expectation_fixture[lib]::crate::User, nominal struct body_direct_closure_expectation_fixture[lib]::crate::User),), Output = nominal struct body_direct_closure_expectation_fixture[lib]::crate::User> @ 12:18-12:19
            body
            expr e0 block s1 => () @ 12:56-12:58


            body b2 fn body_direct_closure_expectation_fixture[lib]::crate::use_it @ 14:1-17:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            - s2 parent s1: v1
            - s3 parent s1: v2, v3
            bindings
            - v0 param attr `attr`: Attr => nominal struct body_direct_closure_expectation_fixture[lib]::crate::Attr @ 14:15-14:19
            - v1 param attrs `attrs` => &mut nominal struct body_direct_closure_expectation_fixture[lib]::crate::AttrVec @ 15:17-15:22
            - v2 param left `left` => nominal struct body_direct_closure_expectation_fixture[lib]::crate::User @ 16:17-16:21
            - v3 param right `right` => nominal struct body_direct_closure_expectation_fixture[lib]::crate::User @ 16:23-16:28
            body
            expr e10 block s1 => () @ 14:27-17:2
              stmt s0 expr; @ 15:5-15:42
                expr e5 call => () @ 15:5-15:41
                  callee
                    expr e0 path with_attrs -> fn body_direct_closure_expectation_fixture[lib]::crate::with_attrs => function item fn body_direct_closure_expectation_fixture[lib]::crate::with_attrs<<unknown>> @ 15:5-15:15
                  arg
                    expr e4 closure s2 (v1) => closure #4 @ 15:16-15:40
                      body
                        expr e3 method_call push -> fn impl AttrVec::push => () @ 15:24-15:40
                          receiver
                            expr e1 path attrs -> local v1 => &mut nominal struct body_direct_closure_expectation_fixture[lib]::crate::AttrVec @ 15:24-15:29
                          arg
                            expr e2 path attr -> local v0 => nominal struct body_direct_closure_expectation_fixture[lib]::crate::Attr @ 15:35-15:39
              stmt s1 expr; @ 16:5-16:37
                expr e9 call => () @ 16:5-16:36
                  callee
                    expr e6 path with_pair -> fn body_direct_closure_expectation_fixture[lib]::crate::with_pair => function item fn body_direct_closure_expectation_fixture[lib]::crate::with_pair<<unknown>> @ 16:5-16:14
                  arg
                    expr e8 closure s3 (v2, v3) => closure #8 @ 16:15-16:35
                      body
                        expr e7 path left -> local v2 => nominal struct body_direct_closure_expectation_fixture[lib]::crate::User @ 16:31-16:35


            body b3 fn impl AttrVec::push @ 6:5-6:42
            scopes
            - s0 parent <none>: v0, v1
            - s1 parent s0: <none>
            bindings
            - v0 self_param self `&mut self` => &mut nominal struct body_direct_closure_expectation_fixture[lib]::crate::AttrVec @ 6:17-6:26
            - v1 param attr `attr`: Attr => nominal struct body_direct_closure_expectation_fixture[lib]::crate::Attr @ 6:28-6:32
            body
            expr e0 block s1 => () @ 6:40-6:42


            package core

            core [lib]
            skipped

            package std

            std [lib]
            skipped
        "#]],
    );
}

#[test]
fn treats_equivalent_fn_trait_bounds_as_one_closure_expectation() {
    check_project_body_ir_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "body_equivalent_callable_bounds_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Name;

pub fn invoke<F>(f: F)
where
    F: FnMut(User) -> Name + FnOnce(User) -> Name,
{}

pub fn use_it() {
    invoke(|user| Name);
}
"#,
        expect![[r#"
            package alloc

            alloc [lib]
            skipped

            package body_equivalent_callable_bounds_fixture

            body_equivalent_callable_bounds_fixture [lib]
            body b0 fn body_equivalent_callable_bounds_fixture[lib]::crate::invoke @ 4:1-7:3
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 param f `f`: F => param F @ 4:18-4:19
            body
            expr e0 block s1 => () @ 7:1-7:3


            body b1 fn body_equivalent_callable_bounds_fixture[lib]::crate::use_it @ 9:1-11:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            - s2 parent s1: v0
            bindings
            - v0 param user `user` => nominal struct body_equivalent_callable_bounds_fixture[lib]::crate::User @ 10:13-10:17
            body
            expr e4 block s1 => () @ 9:17-11:2
              stmt s0 expr; @ 10:5-10:25
                expr e3 call => () @ 10:5-10:24
                  callee
                    expr e0 path invoke -> fn body_equivalent_callable_bounds_fixture[lib]::crate::invoke => function item fn body_equivalent_callable_bounds_fixture[lib]::crate::invoke<<unknown>> @ 10:5-10:11
                  arg
                    expr e2 closure s2 (v0) => closure #2 @ 10:12-10:23
                      body
                        expr e1 path Name -> struct body_equivalent_callable_bounds_fixture[lib]::crate::Name => nominal struct body_equivalent_callable_bounds_fixture[lib]::crate::Name @ 10:19-10:23


            package core

            core [lib]
            skipped

            package std

            std [lib]
            skipped
        "#]],
    );
}

#[test]
fn callable_evidence_flows_through_function_items_and_nested_impls() {
    check_project_body_ir_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "body_solver_callable_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Name;

pub trait Produces {
    type Output;
}

pub struct Adapter<F> {
    value: F,
}

impl<F, R> Produces for Adapter<F>
where
    F: FnOnce(User) -> R,
{
    type Output = R;
}

pub fn adapter<F>(value: F) -> Adapter<F> {
    loop {}
}

pub fn project<F: Produces>(value: F) -> F::Output {
    loop {}
}

pub fn invoke<F>(value: F) -> Name
where
    F: FnOnce(User) -> Name,
{
    loop {}
}

pub fn make_name(user: User) -> Name {
    Name
}

pub fn use_it() {
    let from_closure = project(adapter(|user| Name));
    let from_function = invoke(make_name);
    from_closure;
    from_function;
}
"#,
        expect![[r#"
            package alloc

            alloc [lib]
            skipped

            package body_solver_callable_fixture

            body_solver_callable_fixture [lib]
            body b0 fn body_solver_callable_fixture[lib]::crate::adapter @ 19:1-21:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            - s2 parent s1: <none>
            bindings
            - v0 param value `value`: F => param F @ 19:19-19:24
            body
            expr e2 block s1 => nominal struct body_solver_callable_fixture[lib]::crate::Adapter<param F> @ 19:43-21:2
              tail
                expr e1 loop => nominal struct body_solver_callable_fixture[lib]::crate::Adapter<param F> @ 20:5-20:12
                  body
                    expr e0 block s2 => () @ 20:10-20:12


            body b1 fn body_solver_callable_fixture[lib]::crate::project @ 23:1-25:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            - s2 parent s1: <none>
            bindings
            - v0 param value `value`: F => param F @ 23:29-23:34
            body
            expr e2 block s1 => projection type trait body_solver_callable_fixture[lib]::crate::Produces::Output<param F> @ 23:52-25:2
              tail
                expr e1 loop => projection type trait body_solver_callable_fixture[lib]::crate::Produces::Output<param F> @ 24:5-24:12
                  body
                    expr e0 block s2 => () @ 24:10-24:12


            body b2 fn body_solver_callable_fixture[lib]::crate::invoke @ 27:1-32:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            - s2 parent s1: <none>
            bindings
            - v0 param value `value`: F => param F @ 27:18-27:23
            body
            expr e2 block s1 => nominal struct body_solver_callable_fixture[lib]::crate::Name @ 30:1-32:2
              tail
                expr e1 loop => nominal struct body_solver_callable_fixture[lib]::crate::Name @ 31:5-31:12
                  body
                    expr e0 block s2 => () @ 31:10-31:12


            body b3 fn body_solver_callable_fixture[lib]::crate::make_name @ 34:1-36:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 param user `user`: User => nominal struct body_solver_callable_fixture[lib]::crate::User @ 34:18-34:22
            body
            expr e1 block s1 => nominal struct body_solver_callable_fixture[lib]::crate::Name @ 34:38-36:2
              tail
                expr e0 path Name -> struct body_solver_callable_fixture[lib]::crate::Name => nominal struct body_solver_callable_fixture[lib]::crate::Name @ 35:5-35:9


            body b4 fn body_solver_callable_fixture[lib]::crate::use_it @ 38:1-43:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v1, v2
            - s2 parent s1: v0
            bindings
            - v0 param user `user` => nominal struct body_solver_callable_fixture[lib]::crate::User @ 39:41-39:45
            - v1 let from_closure `from_closure` => nominal struct body_solver_callable_fixture[lib]::crate::Name @ 39:9-39:21
            - v2 let from_function `from_function` => nominal struct body_solver_callable_fixture[lib]::crate::Name @ 40:9-40:22
            body
            expr e11 block s1 => () @ 38:17-43:2
              stmt s0 let v1 @ 39:5-39:54
                initializer
                  expr e5 call => nominal struct body_solver_callable_fixture[lib]::crate::Name @ 39:24-39:53
                    callee
                      expr e0 path project -> fn body_solver_callable_fixture[lib]::crate::project => function item fn body_solver_callable_fixture[lib]::crate::project<<unknown>> @ 39:24-39:31
                    arg
                      expr e4 call => nominal struct body_solver_callable_fixture[lib]::crate::Adapter<closure #3> @ 39:32-39:52
                        callee
                          expr e1 path adapter -> fn body_solver_callable_fixture[lib]::crate::adapter => function item fn body_solver_callable_fixture[lib]::crate::adapter<<unknown>> @ 39:32-39:39
                        arg
                          expr e3 closure s2 (v0) => closure #3 @ 39:40-39:51
                            body
                              expr e2 path Name -> struct body_solver_callable_fixture[lib]::crate::Name => nominal struct body_solver_callable_fixture[lib]::crate::Name @ 39:47-39:51
              stmt s1 let v2 @ 40:5-40:43
                initializer
                  expr e8 call => nominal struct body_solver_callable_fixture[lib]::crate::Name @ 40:25-40:42
                    callee
                      expr e6 path invoke -> fn body_solver_callable_fixture[lib]::crate::invoke => function item fn body_solver_callable_fixture[lib]::crate::invoke<<unknown>> @ 40:25-40:31
                    arg
                      expr e7 path make_name -> fn body_solver_callable_fixture[lib]::crate::make_name => function item fn body_solver_callable_fixture[lib]::crate::make_name @ 40:32-40:41
              stmt s2 expr; @ 41:5-41:18
                expr e9 path from_closure -> local v1 => nominal struct body_solver_callable_fixture[lib]::crate::Name @ 41:5-41:17
              stmt s3 expr; @ 42:5-42:19
                expr e10 path from_function -> local v2 => nominal struct body_solver_callable_fixture[lib]::crate::Name @ 42:5-42:18


            package core

            core [lib]
            skipped

            package std

            std [lib]
            skipped
        "#]],
    );
}

#[test]
fn callable_evidence_flows_through_generic_function_pointers() {
    // A function pointer introduces its own Chalk binder. The surrounding function's `R` must be
    // shifted through that binder before the outer `FnOnce` obligation substitutes its variables.
    check_project_body_ir_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "body_function_pointer_callable_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn invoke<F, R>(value: F) -> R
where
    F: FnOnce(User) -> R,
{
    loop {}
}

pub fn forward<R>(callback: fn(User) -> R) -> R {
    invoke(callback)
}
"#,
        expect![[r#"
            package alloc

            alloc [lib]
            skipped

            package body_function_pointer_callable_fixture

            body_function_pointer_callable_fixture [lib]
            body b0 fn body_function_pointer_callable_fixture[lib]::crate::invoke @ 3:1-8:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            - s2 parent s1: <none>
            bindings
            - v0 param value `value`: F => param F @ 3:21-3:26
            body
            expr e2 block s1 => param R @ 6:1-8:2
              tail
                expr e1 loop => param R @ 7:5-7:12
                  body
                    expr e0 block s2 => () @ 7:10-7:12


            body b1 fn body_function_pointer_callable_fixture[lib]::crate::forward @ 10:1-12:2
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            bindings
            - v0 param callback `callback`: fn(User) -> R => fn(nominal struct body_function_pointer_callable_fixture[lib]::crate::User) -> param R @ 10:19-10:27
            body
            expr e3 block s1 => param R @ 10:49-12:2
              tail
                expr e2 call => param R @ 11:5-11:21
                  callee
                    expr e0 path invoke -> fn body_function_pointer_callable_fixture[lib]::crate::invoke => function item fn body_function_pointer_callable_fixture[lib]::crate::invoke<<unknown>, <unknown>> @ 11:5-11:11
                  arg
                    expr e1 path callback -> local v0 => fn(nominal struct body_function_pointer_callable_fixture[lib]::crate::User) -> param R @ 11:12-11:20


            package core

            core [lib]
            skipped

            package std

            std [lib]
            skipped
        "#]],
    );
}

#[test]
fn never_closure_satisfies_generic_callable_output() {
    check_project_body_ir_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "body_never_closure_callable_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Error;
pub struct ReportedError;

pub trait BuildFrom<T> {}

pub trait Convert<T> {
    fn convert(self) -> T;
}

impl<T, U> Convert<U> for T
where
    U: BuildFrom<T>,
{
    fn convert(self) -> U {
        loop {}
    }
}

impl BuildFrom<Error> for ReportedError {}

pub enum Outcome<T, E> {
    Ok(T),
    Err(E),
}

impl<T, E> Outcome<T, E> {
    pub fn unwrap_or_else<F>(self, fallback: F) -> T
    where
        F: FnOnce(E) -> T,
    {
        loop {}
    }
}

pub fn source() -> Outcome<User, Error> {
    loop {}
}

pub fn abort() -> ! {
    loop {}
}

pub fn report(_: &ReportedError) {}

pub fn use_it() -> User {
    source().unwrap_or_else(|error| {
        report(&error.convert());
        abort()
    })
}
"#,
        expect![[r#"
            package alloc

            alloc [lib]
            skipped

            package body_never_closure_callable_fixture

            body_never_closure_callable_fixture [lib]
            body b0 fn body_never_closure_callable_fixture[lib]::crate::source @ 36:1-38:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            - s2 parent s1: <none>
            bindings
            body
            expr e2 block s1 => nominal enum body_never_closure_callable_fixture[lib]::crate::Outcome<nominal struct body_never_closure_callable_fixture[lib]::crate::User, nominal struct body_never_closure_callable_fixture[lib]::crate::Error> @ 36:41-38:2
              tail
                expr e1 loop => nominal enum body_never_closure_callable_fixture[lib]::crate::Outcome<nominal struct body_never_closure_callable_fixture[lib]::crate::User, nominal struct body_never_closure_callable_fixture[lib]::crate::Error> @ 37:5-37:12
                  body
                    expr e0 block s2 => () @ 37:10-37:12


            body b1 fn body_never_closure_callable_fixture[lib]::crate::abort @ 40:1-42:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            - s2 parent s1: <none>
            bindings
            body
            expr e2 block s1 => ! @ 40:21-42:2
              tail
                expr e1 loop => ! @ 41:5-41:12
                  body
                    expr e0 block s2 => () @ 41:10-41:12


            body b2 fn body_never_closure_callable_fixture[lib]::crate::report @ 44:1-44:36
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e0 block s1 => () @ 44:34-44:36


            body b3 fn body_never_closure_callable_fixture[lib]::crate::use_it @ 46:1-51:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            - s2 parent s1: v0
            - s3 parent s2: <none>
            bindings
            - v0 param error `error` => nominal struct body_never_closure_callable_fixture[lib]::crate::Error @ 47:30-47:35
            body
            expr e12 block s1 => nominal struct body_never_closure_callable_fixture[lib]::crate::User @ 46:25-51:2
              tail
                expr e11 method_call unwrap_or_else -> fn impl Outcome<T, E>::unwrap_or_else => nominal struct body_never_closure_callable_fixture[lib]::crate::User @ 47:5-50:7
                  receiver
                    expr e1 call => nominal enum body_never_closure_callable_fixture[lib]::crate::Outcome<nominal struct body_never_closure_callable_fixture[lib]::crate::User, nominal struct body_never_closure_callable_fixture[lib]::crate::Error> @ 47:5-47:13
                      callee
                        expr e0 path source -> fn body_never_closure_callable_fixture[lib]::crate::source => function item fn body_never_closure_callable_fixture[lib]::crate::source @ 47:5-47:11
                  arg
                    expr e10 closure s2 (v0) => closure #10 @ 47:29-50:6
                      body
                        expr e9 block s3 => nominal struct body_never_closure_callable_fixture[lib]::crate::User @ 47:37-50:6
                          stmt s0 expr; @ 48:9-48:34
                            expr e6 call => () @ 48:9-48:33
                              callee
                                expr e2 path report -> fn body_never_closure_callable_fixture[lib]::crate::report => function item fn body_never_closure_callable_fixture[lib]::crate::report @ 48:9-48:15
                              arg
                                expr e5 wrapper ref => &nominal struct body_never_closure_callable_fixture[lib]::crate::ReportedError @ 48:16-48:32
                                  inner
                                    expr e4 method_call convert => nominal struct body_never_closure_callable_fixture[lib]::crate::ReportedError @ 48:17-48:32
                                      receiver
                                        expr e3 path error -> local v0 => nominal struct body_never_closure_callable_fixture[lib]::crate::Error @ 48:17-48:22
                          tail
                            expr e8 call => ! @ 49:9-49:16
                              callee
                                expr e7 path abort -> fn body_never_closure_callable_fixture[lib]::crate::abort => function item fn body_never_closure_callable_fixture[lib]::crate::abort @ 49:9-49:14


            body b4 fn impl Convert<U> for T::convert @ 15:5-17:6
            scopes
            - s0 parent <none>: v0
            - s1 parent s0: <none>
            - s2 parent s1: <none>
            bindings
            - v0 self_param self `self` => param T @ 15:16-15:20
            body
            expr e2 block s1 => param U @ 15:27-17:6
              tail
                expr e1 loop => param U @ 16:9-16:16
                  body
                    expr e0 block s2 => () @ 16:14-16:16


            body b5 fn impl Outcome<T, E>::unwrap_or_else @ 28:5-33:6
            scopes
            - s0 parent <none>: v0, v1
            - s1 parent s0: <none>
            - s2 parent s1: <none>
            bindings
            - v0 self_param self `self` => nominal enum body_never_closure_callable_fixture[lib]::crate::Outcome<param T, param E> @ 28:30-28:34
            - v1 param fallback `fallback`: F => param F @ 28:36-28:44
            body
            expr e2 block s1 => param T @ 31:5-33:6
              tail
                expr e1 loop => param T @ 32:9-32:16
                  body
                    expr e0 block s2 => () @ 32:14-32:16


            package core

            core [lib]
            skipped

            package std

            std [lib]
            skipped
        "#]],
    );
}
