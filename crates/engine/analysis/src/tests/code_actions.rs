use expect_test::expect;

use super::utils::{
    AnalysisQuery, check_analysis_queries, check_analysis_queries_with_fake_sysroot,
};

#[test]
fn implements_required_trait_members_with_concrete_substitution() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_trait_member_actions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
trait Service<T> {
    type Output;
    type Defaulted = T;

    const REQUIRED: T;
    const DEFAULTED: T = loop {};

    fn required(&self, value: T) -> Self::Output;
    fn defaulted(&self) -> T { loop {} }
}

struct Worker;

impl Service$action$<u8> for Worker {
}
"#,
        &[AnalysisQuery::code_actions(
            "implement missing members",
            "action",
        )],
        expect![[r#"
            implement missing members
            - quickfix Implement missing trait members
              preferred: true
              result:
                trait Service<T> {
                    type Output;
                    type Defaulted = T;

                    const REQUIRED: T;
                    const DEFAULTED: T = loop {};

                    fn required(&self, value: T) -> Self::Output;
                    fn defaulted(&self) -> T { loop {} }
                }

                struct Worker;

                impl Service<u8> for Worker {
                    type Output = ();

                    const REQUIRED: u8 = todo!();

                    fn required(&self, value: u8) -> Self::Output {
                        todo!()
                    }
                }
        "#]],
    );
}

#[test]
fn trait_member_action_uses_declared_type_names_without_adding_imports() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_trait_member_type_paths"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod protocol {
    pub struct Request;

    pub trait Service {
        fn handle(&self, request: Request) -> Request;
    }
}

struct Worker;

impl protocol::Service for Worker {$action$
}
"#,
        &[AnalysisQuery::code_actions(
            "out-of-scope signature type",
            "action",
        )],
        expect![[r#"
            out-of-scope signature type
            - quickfix Implement missing trait members
              preferred: true
              result:
                mod protocol {
                    pub struct Request;

                    pub trait Service {
                        fn handle(&self, request: Request) -> Request;
                    }
                }

                struct Worker;

                impl protocol::Service for Worker {
                    fn handle(&self, request: Request) -> Request {
                        todo!()
                    }
                }
        "#]],
    );
}

#[test]
fn complete_and_inherent_impls_have_no_trait_member_action() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completed_trait_member_actions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
trait Service {
    fn required(&self);
}

struct Worker;

impl Service for Worker {
    fn required(&self) {$complete$}
}

impl Worker {$inherent$
}
"#,
        &[
            AnalysisQuery::code_actions("complete trait impl", "complete"),
            AnalysisQuery::code_actions("inherent impl", "inherent"),
        ],
        expect![[r#"
            complete trait impl

            inherent impl
        "#]],
    );
}

#[test]
fn imports_exact_unresolved_type_and_value_names() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_import_actions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod models {
    pub struct User;
    pub struct Client;

    pub fn make_user() -> User { User }

    impl Client {
        pub fn new() -> Self { Self }
    }
}

fn load() {
    let _: User$type_action$;
    let _ = make_user$value_action$();
    let _ = Client$qualified_root_action$::new();
}
"#,
        &[
            AnalysisQuery::code_actions("unresolved type", "type_action"),
            AnalysisQuery::code_actions("unresolved value", "value_action"),
            AnalysisQuery::code_actions("unresolved qualified root", "qualified_root_action"),
        ],
        expect![[r#"
            unresolved type
            - quickfix Import `crate::models::User`
              preferred: true
              result:
                use crate::models::User;

                mod models {
                    pub struct User;
                    pub struct Client;

                    pub fn make_user() -> User { User }

                    impl Client {
                        pub fn new() -> Self { Self }
                    }
                }

                fn load() {
                    let _: User;
                    let _ = make_user();
                    let _ = Client::new();
                }

            unresolved value
            - quickfix Import `crate::models::make_user`
              preferred: true
              result:
                use crate::models::make_user;

                mod models {
                    pub struct User;
                    pub struct Client;

                    pub fn make_user() -> User { User }

                    impl Client {
                        pub fn new() -> Self { Self }
                    }
                }

                fn load() {
                    let _: User;
                    let _ = make_user();
                    let _ = Client::new();
                }

            unresolved qualified root
            - quickfix Import `crate::models::Client`
              preferred: true
              result:
                use crate::models::Client;

                mod models {
                    pub struct User;
                    pub struct Client;

                    pub fn make_user() -> User { User }

                    impl Client {
                        pub fn new() -> Self { Self }
                    }
                }

                fn load() {
                    let _: User;
                    let _ = make_user();
                    let _ = Client::new();
                }
        "#]],
    );
}

#[test]
fn imports_std_type_used_as_qualified_path_root() {
    check_analysis_queries_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_std_import_actions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
fn load() {
    let _ = Arc$action$::new(1);
}
"#,
        &[
            AnalysisQuery::code_actions("unresolved std path root", "action")
                .in_lib("analysis_std_import_actions"),
        ],
        expect![[r#"
            unresolved std path root
            - quickfix Import `alloc::sync::Arc`
              preferred: true
              result:
                use alloc::sync::Arc;

                fn load() {
                    let _ = Arc::new(1);
                }
        "#]],
    );
}

#[test]
fn import_action_distinguishes_private_imports_from_reexports() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["catalog", "app"]
resolver = "3"

//- /catalog/Cargo.toml
[package]
name = "catalog"
version = "0.1.0"
edition = "2024"

//- /catalog/src/lib.rs
pub mod collections {
    pub struct BTreeMap;
    pub struct HashMap;
}

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
catalog = { path = "../catalog" }

//- /app/src/lib.rs
use catalog::collections::HashMap;
pub(crate) use catalog::collections::BTreeMap;

mod nested {
    fn load_private_import() {
        let _ = HashMap$private$::new();
    }

    fn load_reexport() {
        let _ = BTreeMap$reexport$::new();
    }
}
"#,
        &[
            AnalysisQuery::code_actions("private parent import", "private").in_lib("app"),
            AnalysisQuery::code_actions("crate re-export", "reexport").in_lib("app"),
        ],
        expect![[r#"
            private parent import
            - quickfix Import `catalog::collections::HashMap`
              preferred: true
              result:
                use catalog::collections::HashMap;
                pub(crate) use catalog::collections::BTreeMap;

                mod nested {
                    use catalog::collections::HashMap;

                    fn load_private_import() {
                        let _ = HashMap::new();
                    }

                    fn load_reexport() {
                        let _ = BTreeMap::new();
                    }
                }

            crate re-export
            - quickfix Import `crate::BTreeMap`
              preferred: true
              result:
                use catalog::collections::HashMap;
                pub(crate) use catalog::collections::BTreeMap;

                mod nested {
                    use crate::BTreeMap;

                    fn load_private_import() {
                        let _ = HashMap::new();
                    }

                    fn load_reexport() {
                        let _ = BTreeMap::new();
                    }
                }
        "#]],
    );
}

#[test]
fn import_actions_preserve_ambiguity_and_require_explicit_invocation() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_ambiguous_import_actions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod alpha { pub struct User; }
mod beta { pub struct User; }

fn load() {
    let _: User$invoked$;
    let _: User$automatic$;
}
"#,
        &[
            AnalysisQuery::code_actions("explicit import discovery", "invoked"),
            AnalysisQuery::automatic_code_actions("automatic discovery", "automatic"),
        ],
        expect![[r#"
            explicit import discovery
            - quickfix Import `crate::alpha::User`
              preferred: false
              result:
                use crate::alpha::User;

                mod alpha { pub struct User; }
                mod beta { pub struct User; }

                fn load() {
                    let _: User;
                    let _: User;
                }

            - quickfix Import `crate::beta::User`
              preferred: false
              result:
                use crate::beta::User;

                mod alpha { pub struct User; }
                mod beta { pub struct User; }

                fn load() {
                    let _: User;
                    let _: User;
                }

            automatic discovery
        "#]],
    );
}

#[test]
fn import_action_is_absent_for_a_resolved_name() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_resolved_import_actions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
struct User;

fn load() {
    let _: User$resolved$;
}
"#,
        &[AnalysisQuery::code_actions("resolved type", "resolved")],
        expect![[r#"
            resolved type
        "#]],
    );
}

#[test]
fn replaces_resolved_qualified_paths_with_imports() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_qualified_path_actions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod models {
    pub struct User<T>(T);

    pub fn make_user() -> User<u8> { User(0) }
}

fn load() {
    let _: crate::models::User$type_action$<u8>;
    let _ = crate::models::make_user$value_action$();
}
"#,
        &[
            AnalysisQuery::code_actions("qualified type", "type_action"),
            AnalysisQuery::code_actions("qualified value", "value_action"),
        ],
        expect![[r#"
            qualified type
            - refactor.rewrite Replace qualified path with `use`
              preferred: false
              result:
                use crate::models::User;

                mod models {
                    pub struct User<T>(T);

                    pub fn make_user() -> User<u8> { User(0) }
                }

                fn load() {
                    let _: User<u8>;
                    let _ = crate::models::make_user();
                }

            qualified value
            - refactor.rewrite Replace qualified path with `use`
              preferred: false
              result:
                use crate::models::make_user;

                mod models {
                    pub struct User<T>(T);

                    pub fn make_user() -> User<u8> { User(0) }
                }

                fn load() {
                    let _: crate::models::User<u8>;
                    let _ = make_user();
                }
        "#]],
    );
}

#[test]
fn qualified_path_reuses_an_existing_import() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_existing_import_path_actions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod models { pub struct User; }
use crate::models::User;

fn load() {
    let _: crate::models::User$action$;
}
"#,
        &[AnalysisQuery::code_actions("existing import", "action")],
        expect![[r#"
            existing import
            - refactor.rewrite Replace qualified path with `use`
              preferred: false
              result:
                mod models { pub struct User; }
                use crate::models::User;

                fn load() {
                    let _: User;
                }
        "#]],
    );
}

#[test]
fn qualified_path_declines_conflicts_and_associated_items() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_excluded_qualified_path_actions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod models {
    pub struct User;
    pub enum State { Ready }
    impl User { pub fn new() -> Self { Self } }
}

fn load() {
    struct User;
    let _: crate::models::User$conflict$;
    let _ = crate::models::User::new$associated$();
    let _ = crate::models::State::Ready$variant$;
}

struct Worker;

impl Worker {
    fn new() -> Self { Self }

    fn self_path() {
        let _ = Self::new$self_type$();
    }
}

trait Named { type Output; }
impl Named for Worker { type Output = (); }

fn anchored(_: <Worker as Named>::Output$anchor$) {}

fn body_local() {
    mod local { pub struct User; }
    let _: local::User$body_local$;
}
"#,
        &[
            AnalysisQuery::code_actions("short-name conflict", "conflict"),
            AnalysisQuery::code_actions("associated item", "associated"),
            AnalysisQuery::code_actions("enum variant", "variant"),
            AnalysisQuery::code_actions("Self path", "self_type"),
            AnalysisQuery::code_actions("qualified type anchor", "anchor"),
            AnalysisQuery::code_actions("body-local module", "body_local"),
        ],
        expect![[r#"
            short-name conflict

            associated item

            enum variant

            Self path

            qualified type anchor

            body-local module
        "#]],
    );
}

#[test]
fn trait_member_action_formats_a_nested_one_line_impl_and_skips_unresolved_traits() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_nested_trait_member_actions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
trait Service {
    type Output;
    fn run(&self) -> Self::Output;
}

struct Worker;

mod nested {
    impl super::Service for super::Worker {$nested$}
}

impl Unknown for Worker {$unresolved$}
"#,
        &[
            AnalysisQuery::code_actions("nested one-line impl", "nested"),
            AnalysisQuery::code_actions("unresolved trait", "unresolved"),
        ],
        expect![[r#"
            nested one-line impl
            - quickfix Implement missing trait members
              preferred: true
              result:
                trait Service {
                    type Output;
                    fn run(&self) -> Self::Output;
                }

                struct Worker;

                mod nested {
                    impl super::Service for super::Worker {
                        type Output = ();

                        fn run(&self) -> Self::Output {
                            todo!()
                        }
                    }
                }

                impl Unknown for Worker {}

            unresolved trait
        "#]],
    );
}

#[test]
fn imports_one_character_and_reexported_names_into_the_innermost_module() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_import_context_actions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod models {
    pub struct X;
    pub struct User;
}

mod exports {
    pub use crate::models::User as PublicUser;
}

mod nested {
    fn one_character(_: X$one$) {}
}

fn reexport(_: PublicUser$reexport$) {}
"#,
        &[
            AnalysisQuery::code_actions("one-character nested import", "one"),
            AnalysisQuery::code_actions("re-export import", "reexport"),
        ],
        expect![[r#"
            one-character nested import
            - quickfix Import `crate::models::X`
              preferred: true
              result:
                mod models {
                    pub struct X;
                    pub struct User;
                }

                mod exports {
                    pub use crate::models::User as PublicUser;
                }

                mod nested {
                    use crate::models::X;

                    fn one_character(_: X) {}
                }

                fn reexport(_: PublicUser) {}

            re-export import
            - quickfix Import `crate::exports::PublicUser`
              preferred: true
              result:
                use crate::exports::PublicUser;

                mod models {
                    pub struct X;
                    pub struct User;
                }

                mod exports {
                    pub use crate::models::User as PublicUser;
                }

                mod nested {
                    fn one_character(_: X) {}
                }

                fn reexport(_: PublicUser) {}
        "#]],
    );
}

#[test]
fn import_action_excludes_private_items_and_macro_call_syntax() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_excluded_import_actions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod hidden { struct Secret; }
mod values {
    pub enum State { Ready }

    #[cfg(any())]
    pub struct Disabled;
}

macro_rules! build { () => {}; }

fn load(_: Secret$private$) {
    build$macro_call$!();
    let _ = Ready$variant$;
    let _: Disabled$cfg_disabled$;
}
"#,
        &[
            AnalysisQuery::code_actions("private item", "private"),
            AnalysisQuery::code_actions("macro call", "macro_call"),
            AnalysisQuery::code_actions("enum variant", "variant"),
            AnalysisQuery::code_actions("cfg-disabled item", "cfg_disabled"),
        ],
        expect![[r#"
            private item

            macro call

            enum variant

            cfg-disabled item
        "#]],
    );
}

#[test]
fn qualified_path_supports_self_and_super_roots_but_not_use_items() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_qualified_root_actions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod outer {
    pub struct Local;
    pub mod models { pub struct User; }

    fn local(_: self::Local$self_root$) {}

    mod nested {
        use super::models::User$use_item$;

        fn user(_: super::models::User$super_root$) {}
    }
}
"#,
        &[
            AnalysisQuery::code_actions("use item", "use_item"),
            AnalysisQuery::code_actions("same-module target", "self_root"),
            AnalysisQuery::code_actions("super-root target", "super_root"),
        ],
        expect![[r#"
            use item

            same-module target
            - refactor.rewrite Replace qualified path with `use`
              preferred: false
              result:
                mod outer {
                    pub struct Local;
                    pub mod models { pub struct User; }

                    fn local(_: Local) {}

                    mod nested {
                        use super::models::User;

                        fn user(_: super::models::User) {}
                    }
                }

            super-root target
            - refactor.rewrite Replace qualified path with `use`
              preferred: false
              result:
                mod outer {
                    pub struct Local;
                    pub mod models { pub struct User; }

                    fn local(_: self::Local) {}

                    mod nested {
                        use super::models::User;

                        fn user(_: User) {}
                    }
                }
        "#]],
    );
}

#[test]
fn qualified_path_supports_external_crate_roots() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["catalog", "app"]
resolver = "3"

//- /catalog/Cargo.toml
[package]
name = "catalog"
version = "0.1.0"
edition = "2024"

//- /catalog/src/lib.rs
pub struct External;

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
catalog = { path = "../catalog" }

//- /app/src/lib.rs
fn load(_: catalog::External$action$) {}
"#,
        &[AnalysisQuery::code_actions("external path", "action").in_lib("app")],
        expect![[r#"
            external path
            - refactor.rewrite Replace qualified path with `use`
              preferred: false
              result:
                use catalog::External;

                fn load(_: External) {}
        "#]],
    );
}

#[test]
fn qualified_path_supports_std_roots() {
    check_analysis_queries_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_std_qualified_path_actions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
fn load(_: std::sync::Arc$action$<u8>) {}
"#,
        &[AnalysisQuery::code_actions("std path", "action")
            .in_lib("analysis_std_qualified_path_actions")],
        expect![[r#"
            std path
            - refactor.rewrite Replace qualified path with `use`
              preferred: false
              result:
                use alloc::sync::Arc;

                fn load(_: Arc<u8>) {}
        "#]],
    );
}

#[test]
fn orders_independent_actions_stably_at_the_same_position() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_combined_code_actions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
trait Service {
    fn required(&self);
}

struct Worker;
mod models { pub struct User; }

impl Service for Worker {
    fn helper(_: User$action$) {}
}
"#,
        &[AnalysisQuery::code_actions("combined actions", "action")],
        expect![[r#"
            combined actions
            - quickfix Implement missing trait members
              preferred: true
              result:
                trait Service {
                    fn required(&self);
                }

                struct Worker;
                mod models { pub struct User; }

                impl Service for Worker {
                    fn helper(_: User) {}

                    fn required(&self) {
                        todo!()
                    }
                }

            - quickfix Import `crate::models::User`
              preferred: true
              result:
                use crate::models::User;

                trait Service {
                    fn required(&self);
                }

                struct Worker;
                mod models { pub struct User; }

                impl Service for Worker {
                    fn helper(_: User) {}
                }
        "#]],
    );
}
