use expect_test::expect;

use super::utils::{AnalysisQuery, check_analysis_queries};

#[test]
fn resolves_type_trait_and_trait_method_implementations() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_goto_implementation"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Account;
pub struct UserName;

pub trait Na$impl_trait$med {
    fn na$impl_trait_method$me(&self) -> UserName;
}

impl Us$impl_type$er {
    pub fn n$impl_inherent$ew() -> Self {
        User
    }
}

impl Named for User {
    fn na$impl_method$me(&self) -> UserName {
        missing()
    }
}

impl Named for Account {
    fn name(&self) -> UserName {
        missing()
    }
}

pub fn use_it(user: User) {
    let _again: User = User::n$impl_inherent_use$ew();
    let _name = user.na$impl_trait_call$me();
    let shared: &&User = &&user;
    let _ref_name = shared.na$impl_trait_ref_call$me();
    let _ = User::na$associated$me(&user);
    let _ = <User as Named>::na$qualified$me(&user);
    let _method = User::na$path$me;
}
"#,
        &[
            AnalysisQuery::goto_impl("goto implementations of type", "impl_type"),
            AnalysisQuery::goto_impl("goto implementations of trait", "impl_trait"),
            AnalysisQuery::goto_impl("goto implementations of trait method", "impl_trait_method"),
            AnalysisQuery::goto_impl("goto implementations from impl method", "impl_method"),
            AnalysisQuery::goto_impl("goto implementations from associated call", "associated"),
            AnalysisQuery::goto_impl("goto implementations from qualified call", "qualified"),
            AnalysisQuery::goto_impl("goto implementations from method path", "path"),
            AnalysisQuery::goto_impl("goto inherent implementation", "impl_inherent"),
            AnalysisQuery::goto_impl("goto inherent implementation use", "impl_inherent_use"),
            AnalysisQuery::goto_impl("goto trait implementation from call", "impl_trait_call"),
            AnalysisQuery::goto_impl(
                "goto trait implementation from reference call",
                "impl_trait_ref_call",
            ),
        ],
        expect![[r#"
            goto implementations of type
            - impl Named for User @ 15:1-19:2
            - impl User @ 9:1-13:2

            goto implementations of trait
            - impl Named for Account @ 21:1-25:2
            - impl Named for User @ 15:1-19:2

            goto implementations of trait method
            - fn name @ 16:8-16:12
            - fn name @ 22:8-22:12

            goto implementations from impl method
            - fn name @ 16:8-16:12
            - fn name @ 22:8-22:12

            goto implementations from associated call
            - fn name @ 16:8-16:12
            - fn name @ 22:8-22:12

            goto implementations from qualified call
            - fn name @ 16:8-16:12
            - fn name @ 22:8-22:12

            goto implementations from method path
            - fn name @ 16:8-16:12
            - fn name @ 22:8-22:12

            goto inherent implementation
            - fn new @ 10:12-10:15

            goto inherent implementation use
            - fn new @ 10:12-10:15

            goto trait implementation from call
            - fn name @ 16:8-16:12
            - fn name @ 22:8-22:12

            goto trait implementation from reference call
            - fn name @ 16:8-16:12
            - fn name @ 22:8-22:12
        "#]],
    );
}

#[test]
fn resolves_body_local_type_implementations() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_goto_implementation"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    struct Us$impl_local_type$er;

    impl User {
        fn id(&self) {}
    }

    let us$impl_local_binding$er: User;
}
"#,
        &[
            AnalysisQuery::goto_impl(
                "goto body-local implementations from type",
                "impl_local_type",
            ),
            AnalysisQuery::goto_impl(
                "goto body-local implementations from binding",
                "impl_local_binding",
            ),
        ],
        expect![[r#"
            goto body-local implementations from type
            - impl User @ 4:5-6:6

            goto body-local implementations from binding
            - impl User @ 4:5-6:6
        "#]],
    );
}

#[test]
fn finds_trait_method_implementations_across_receiver_generic_args() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_goto_implementation_trait_impl_generics"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Account;
pub struct Wrapper<T>(T);

pub trait Named {
    fn name(&self);
}

impl Named for Wrapper<User> {
    fn name(&self) {}
}

impl Named for Wrapper<Account> {
    fn name(&self) {}
}

pub fn use_it(account: Wrapper<Account>) {
    account.na$impl_account_call$me();
}
"#,
        &[AnalysisQuery::goto_impl(
            "goto trait implementation from generic receiver call",
            "impl_account_call",
        )],
        expect![[r#"
            goto trait implementation from generic receiver call
            - fn name @ 10:8-10:12
            - fn name @ 14:8-14:12
        "#]],
    );
}

#[test]
fn shares_body_local_impl_discovery_between_traits_and_methods() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_local_trait_implementations"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Global { fn run(&self); }
pub struct Service;
impl Global for Service { fn run(&self) {} }

pub fn use_it() {
    trait Lo$local_trait$cal { fn ru$local_method$n(&self); }
    struct User;
    struct Account;
    impl Local for User { fn ru$local_impl_method$n(&self) {} }
    impl Local for Account { fn run(&self) {} }
    impl Glo$global_trait$bal for User { fn ru$global_impl_method$n(&self) {} }
    impl Global for Account { fn run(&self) {} }

    fn nested(user: User) {
        <User as Local>::ru$local_call$n(&user);
        <User as Global>::ru$global_call$n(&user);
        let _method = Service::ru$global_path$n;
    }
}
"#,
        &[
            AnalysisQuery::goto_impl("local trait impl blocks", "local_trait"),
            AnalysisQuery::goto_impl("local trait method", "local_method"),
            AnalysisQuery::goto_impl("local impl method", "local_impl_method"),
            AnalysisQuery::goto_impl("local trait from nested body", "local_call"),
            AnalysisQuery::goto_impl("global trait in local impl header", "global_trait"),
            AnalysisQuery::goto_impl("global trait local impl method", "global_impl_method"),
            AnalysisQuery::goto_impl("global trait from nested body", "global_call"),
            AnalysisQuery::goto_impl("global trait standalone method path", "global_path"),
        ],
        expect![[r#"
            local trait impl blocks
            - impl Local for Account @ 10:5-10:48
            - impl Local for User @ 9:5-9:45

            local trait method
            - fn run @ 9:30-9:33
            - fn run @ 10:33-10:36

            local impl method
            - fn run @ 9:30-9:33
            - fn run @ 10:33-10:36

            local trait from nested body
            - fn run @ 9:30-9:33
            - fn run @ 10:33-10:36

            global trait in local impl header
            - impl Global for Account @ 12:5-12:49
            - impl Global for Service @ 3:1-3:45
            - impl Global for User @ 11:5-11:46

            global trait local impl method
            - fn run @ 3:30-3:33
            - fn run @ 11:31-11:34
            - fn run @ 12:34-12:37

            global trait from nested body
            - fn run @ 3:30-3:33
            - fn run @ 11:31-11:34
            - fn run @ 12:34-12:37

            global trait standalone method path
            - fn run @ 3:30-3:33
            - fn run @ 11:31-11:34
            - fn run @ 12:34-12:37
        "#]],
    );
}

#[test]
fn expands_supertrait_methods_across_receiver_forms() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_supertrait_implementations"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Parent { fn ru$declaration$n(&self); }
pub trait Child: Parent {}
pub struct User;
pub struct Wrapper<T>(T);
impl Parent for User { fn run(&self) {} }
impl Child for User {}
impl<T> Parent for Wrapper<T> { fn run(&self) {} }
impl<T> Parent for &T { fn run(&self) {} }
impl Parent for u8 { fn run(&self) {} }
impl Parent for [u8] { fn run(&self) {} }

pub fn use_it<T: Child>(value: T, wrapped: Wrapper<User>, number: u8, slice: &[u8]) {
    value.ru$supertrait$n();
    wrapped.ru$blanket$n();
    number.ru$primitive$n();
    slice.ru$reference$n();
    <[u8] as Parent>::ru$structural$n(slice);
}
"#,
        &[
            AnalysisQuery::goto_impl("parent method", "declaration"),
            AnalysisQuery::goto_impl("method through supertrait bound", "supertrait"),
            AnalysisQuery::goto_impl("blanket receiver call", "blanket"),
            AnalysisQuery::goto_impl("primitive receiver call", "primitive"),
            AnalysisQuery::goto_impl("reference receiver call", "reference"),
            AnalysisQuery::goto_impl("structural receiver call", "structural"),
        ],
        expect![[r#"
            parent method
            - fn run @ 5:27-5:30
            - fn run @ 7:36-7:39
            - fn run @ 8:28-8:31
            - fn run @ 9:25-9:28
            - fn run @ 10:27-10:30

            method through supertrait bound
            - fn run @ 5:27-5:30
            - fn run @ 7:36-7:39
            - fn run @ 8:28-8:31
            - fn run @ 9:25-9:28
            - fn run @ 10:27-10:30

            blanket receiver call
            - fn run @ 5:27-5:30
            - fn run @ 7:36-7:39
            - fn run @ 8:28-8:31
            - fn run @ 9:25-9:28
            - fn run @ 10:27-10:30

            primitive receiver call
            - fn run @ 5:27-5:30
            - fn run @ 7:36-7:39
            - fn run @ 8:28-8:31
            - fn run @ 9:25-9:28
            - fn run @ 10:27-10:30

            reference receiver call
            - fn run @ 5:27-5:30
            - fn run @ 7:36-7:39
            - fn run @ 8:28-8:31
            - fn run @ 9:25-9:28
            - fn run @ 10:27-10:30

            structural receiver call
            - fn run @ 5:27-5:30
            - fn run @ 7:36-7:39
            - fn run @ 8:28-8:31
            - fn run @ 9:25-9:28
            - fn run @ 10:27-10:30
        "#]],
    );
}

#[test]
fn finds_all_explicit_overrides_and_finishes_default_only_queries() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_default_implementations"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Result;
impl Result { fn other(&self) {} }
pub trait DefaultOnly { fn ma$default_declaration$ke(&self) -> Result { Result } }
pub trait Overridden { fn make(&self) -> Result { Result } }
pub struct User;
pub struct Account;
impl DefaultOnly for User {}
impl Overridden for User {}
impl Overridden for Account { fn make(&self) -> Result { Result } }

pub fn use_it(user: User) {
    let _ = <User as DefaultOnly>::ma$qualified_default$ke(&user);
    let _method = <User as DefaultOnly>::ma$default_path$ke;
    let _ = <User as Overridden>::ma$override$ke(&user);
}
pub fn default_call<T: DefaultOnly>(value: T) { value.ma$dot_default$ke(); }
pub fn override_call<T: Overridden>(value: T) { value.ma$dot_override$ke(); }
"#,
        &[
            AnalysisQuery::goto_impl("default-only declaration", "default_declaration"),
            AnalysisQuery::goto_impl("default-only qualified call", "qualified_default"),
            AnalysisQuery::goto_impl("default-only standalone path", "default_path"),
            AnalysisQuery::goto_impl("default-only dot call", "dot_default"),
            AnalysisQuery::goto_impl("explicit override from inherited default call", "override"),
            AnalysisQuery::goto_impl("explicit override from generic call", "dot_override"),
            AnalysisQuery::goto("definition of inherited default", "qualified_default"),
        ],
        expect![[r#"
            default-only declaration
            - <none>

            default-only qualified call
            - <none>

            default-only standalone path
            - <none>

            default-only dot call
            - <none>

            explicit override from inherited default call
            - fn make @ 9:34-9:38

            explicit override from generic call
            - fn make @ 9:34-9:38

            definition of inherited default
            - fn make @ 3:28-3:32
        "#]],
    );
}

#[test]
fn finds_dependency_and_local_impls_across_trait_generic_arguments() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_dependency_implementations"
version = "0.1.0"
edition = "2024"
[dependencies]
api = { path = "api" }

//- /api/Cargo.toml
[package]
name = "api"
version = "0.1.0"
edition = "2024"

//- /api/src/lib.rs
pub trait Convert<T> { fn convert<U>(&self, input: U) -> T; }
pub struct Remote;
impl Convert<u8> for Remote {
    fn convert<U>(&self, input: U) -> u8 { 0 }
}

//- /src/lib.rs
use api::Convert;
pub struct Local;
impl Convert<u16> for Local { fn con$local_method$vert<U>(&self, input: U) -> u16 { 0 } }
impl Convert<u32> for Local { fn convert<U>(&self, input: U) -> u32 { 0 } }
pub fn use_it(local: Local, remote: api::Remote) {
    let _: u16 = local.con$local$vert(0u8);
    let _: u8 = remote.con$remote$vert(0u16);
    let _ = <Local as Convert<u32>>::con$qualified$vert::<u8>(&local, 0);
}
"#,
        &[
            AnalysisQuery::goto_impl("trait impl method declaration", "local_method")
                .in_lib("analysis_dependency_implementations"),
            AnalysisQuery::goto_impl("local receiver call", "local")
                .in_lib("analysis_dependency_implementations"),
            AnalysisQuery::goto_impl("dependency receiver call", "remote")
                .in_lib("analysis_dependency_implementations"),
            AnalysisQuery::goto_impl("qualified generic call", "qualified")
                .in_lib("analysis_dependency_implementations"),
            AnalysisQuery::goto("selected local definition", "local")
                .in_lib("analysis_dependency_implementations"),
            AnalysisQuery::goto("selected dependency definition", "remote")
                .in_lib("analysis_dependency_implementations"),
        ],
        expect![[r#"
            trait impl method declaration
            - fn convert @ 3:34-3:41
            - fn convert @ 4:34-4:41
            - fn convert @ 4:8-4:15

            local receiver call
            - fn convert @ 3:34-3:41
            - fn convert @ 4:34-4:41
            - fn convert @ 4:8-4:15

            dependency receiver call
            - fn convert @ 3:34-3:41
            - fn convert @ 4:34-4:41
            - fn convert @ 4:8-4:15

            qualified generic call
            - fn convert @ 3:34-3:41
            - fn convert @ 4:34-4:41
            - fn convert @ 4:8-4:15

            selected local definition
            - fn convert @ 3:34-3:41

            selected dependency definition
            - fn convert @ 4:8-4:15
        "#]],
    );
}
