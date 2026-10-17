use expect_test::expect;

use super::utils::{check_analysis, check_analysis_with_fake_sysroot};

#[test]
fn implements_required_trait_members_with_concrete_substitution() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_trait_member_actions
trait Service<T> {
    type Output;
    type Defaulted = T;

    const REQUIRED: T;
    const DEFAULTED: T = loop {};

    fn required(&self, value: T) -> Self::Output;
    fn defaulted(&self) -> T { loop {} }
}

struct Worker;

impl Service$0<u8> for Worker { // code_actions("implement missing members")
}
"#,
        expect![[r#"
            // implement missing members
            quickfix Implement missing trait members
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
fn trait_member_action_for_signature_types_that_need_imports() {
    // `Request` lives in `protocol` and is not imported beside `Worker`. This records the
    // nominal-type qualification/import work still needed for the code this action inserts.
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_trait_member_type_paths
mod protocol {
    pub struct Request;

    pub trait Service {
        fn handle(&self, request: Request) -> Request;
    }
}

struct Worker;

impl protocol::Service for Worker {$0 // code_actions("out-of-scope signature type")
}
"#,
        expect![[r#"
            // out-of-scope signature type
            quickfix Implement missing trait members
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
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_completed_trait_member_actions
trait Service {
    fn required(&self);
}

struct Worker;

impl Service for Worker {
    fn required(&self) {$0} // code_actions("complete trait impl")
}

impl Worker {$0 // code_actions("inherent impl")
}
"#,
        expect![[r#"
            // complete trait impl
            // inherent impl
        "#]],
    );
}

#[test]
fn imports_exact_unresolved_type_and_value_names() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_import_actions
mod models {
    pub struct User;
    pub struct Client;

    pub fn make_user() -> User { User }

    impl Client {
        pub fn new() -> Self { Self }
    }
}

fn load() {
    let _: User$0; // code_actions("unresolved type")
    let _ = make_user$0(); // code_actions("unresolved value")
    let _ = Client$0::new(); // code_actions("unresolved qualified root")
}
"#,
        expect![[r#"
            // unresolved type
            quickfix Import `crate::models::User`
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

            // unresolved value
            quickfix Import `crate::models::make_user`
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

            // unresolved qualified root
            quickfix Import `crate::models::Client`
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
    check_analysis_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:analysis_std_import_actions
fn load() {
    let _ = Arc$0::new(1); // code_actions("unresolved std path root").in_lib("analysis_std_import_actions")
}
"#,
        expect![[r#"
            // unresolved std path root
            quickfix Import `alloc::sync::Arc`
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
    check_analysis(
        r#"
//- /catalog/src/lib.rs crate:catalog root:catalog
pub mod collections {
    pub struct BTreeMap;
    pub struct HashMap;
}

//- /app/src/lib.rs crate:app root:app deps:catalog
use catalog::collections::HashMap;
pub(crate) use catalog::collections::BTreeMap;

mod nested {
    fn load_private_import() {
        let _ = HashMap$0::new(); // code_actions("private parent import").in_lib("app")
    }

    fn load_reexport() {
        let _ = BTreeMap$0::new(); // code_actions("crate re-export").in_lib("app")
    }
}
"#,
        expect![[r#"
            // private parent import
            quickfix Import `catalog::collections::HashMap`
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

            // crate re-export
            quickfix Import `crate::BTreeMap`
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
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_ambiguous_import_actions
mod alpha { pub struct User; }
mod beta { pub struct User; }

fn load() {
    let _: User$0; // code_actions("explicit import discovery")
    let _: User$0; // automatic_code_actions("automatic discovery")
}
"#,
        expect![[r#"
            // explicit import discovery
            quickfix Import `crate::alpha::User`
              preferred: false
              result:
                use crate::alpha::User;

                mod alpha { pub struct User; }
                mod beta { pub struct User; }

                fn load() {
                    let _: User;
                    let _: User;
                }

            quickfix Import `crate::beta::User`
              preferred: false
              result:
                use crate::beta::User;

                mod alpha { pub struct User; }
                mod beta { pub struct User; }

                fn load() {
                    let _: User;
                    let _: User;
                }

            // automatic discovery
        "#]],
    );
}

#[test]
fn import_action_is_absent_for_a_resolved_name() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_resolved_import_actions
struct User;

fn load() {
    let _: User$0; // code_actions("resolved type")
}
"#,
        expect![[r#"
            // resolved type
        "#]],
    );
}

#[test]
fn replaces_resolved_qualified_paths_with_imports() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_qualified_path_actions
mod models {
    pub struct User<T>(T);

    pub fn make_user() -> User<u8> { User(0) }
}

fn load() {
    let _: crate::models::User$0<u8>; // code_actions("qualified type")
    let _ = crate::models::make_user$0(); // code_actions("qualified value")
}
"#,
        expect![[r#"
            // qualified type
            refactor.rewrite Replace qualified path with `use`
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

            // qualified value
            refactor.rewrite Replace qualified path with `use`
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
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_existing_import_path_actions
mod models { pub struct User; }
use crate::models::User;

fn load() {
    let _: crate::models::User$0; // code_actions("existing import")
}
"#,
        expect![[r#"
            // existing import
            refactor.rewrite Replace qualified path with `use`
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
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_excluded_qualified_path_actions
mod models {
    pub struct User;
    pub enum State { Ready }
    impl User { pub fn new() -> Self { Self } }
}

fn load() {
    struct User;
    let _: crate::models::User$0; // code_actions("short-name conflict")
    let _ = crate::models::User::new$0(); // code_actions("associated item")
    let _ = crate::models::State::Ready$0; // code_actions("enum variant")
}

struct Worker;

impl Worker {
    fn new() -> Self { Self }

    fn self_path() {
        let _ = Self::new$0(); // code_actions("Self path")
    }
}

trait Named { type Output; }
impl Named for Worker { type Output = (); }

fn anchored(_: <Worker as Named>::Output$0) {} // code_actions("qualified type anchor")

fn body_local() {
    mod local { pub struct User; }
    let _: local::User$0; // code_actions("body-local module")
}
"#,
        expect![[r#"
            // short-name conflict
            // associated item
            // enum variant
            // Self path
            // qualified type anchor
            // body-local module
        "#]],
    );
}

#[test]
fn trait_member_action_formats_a_nested_one_line_impl_and_skips_unresolved_traits() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_nested_trait_member_actions
trait Service {
    type Output;
    fn run(&self) -> Self::Output;
}

struct Worker;

mod nested {
    impl super::Service for super::Worker {$0} // code_actions("nested one-line impl")
}

impl Unknown for Worker {$0} // code_actions("unresolved trait")
"#,
        expect![[r#"
            // nested one-line impl
            quickfix Implement missing trait members
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

            // unresolved trait
        "#]],
    );
}

#[test]
fn imports_one_character_and_reexported_names_into_the_innermost_module() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_import_context_actions
mod models {
    pub struct X;
    pub struct User;
}

mod exports {
    pub use crate::models::User as PublicUser;
}

mod nested {
    fn one_character(_: X$0) {} // code_actions("one-character nested import")
}

fn reexport(_: PublicUser$0) {} // code_actions("re-export import")
"#,
        expect![[r#"
            // one-character nested import
            quickfix Import `crate::models::X`
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

            // re-export import
            quickfix Import `crate::exports::PublicUser`
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
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_excluded_import_actions
mod hidden { struct Secret; }
mod values {
    pub enum State { Ready }

    #[cfg(any())]
    pub struct Disabled;
}

macro_rules! build { () => {}; }

fn load(_: Secret$0) { // code_actions("private item")
    build$0!(); // code_actions("macro call")
    let _ = Ready$0; // code_actions("enum variant")
    let _: Disabled$0; // code_actions("cfg-disabled item")
}
"#,
        expect![[r#"
            // private item
            // macro call
            // enum variant
            // cfg-disabled item
        "#]],
    );
}

#[test]
fn qualified_path_supports_self_and_super_roots_but_not_use_items() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_qualified_root_actions
mod outer {
    pub struct Local;
    pub mod models { pub struct User; }

    fn local(_: self::Local$0) {} // code_actions("same-module target")

    mod nested {
        use super::models::User$0; // code_actions("use item")

        fn user(_: super::models::User$0) {} // code_actions("super-root target")
    }
}
"#,
        expect![[r#"
            // same-module target
            refactor.rewrite Replace qualified path with `use`
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

            // use item

            // super-root target
            refactor.rewrite Replace qualified path with `use`
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
    check_analysis(
        r#"
//- /catalog/src/lib.rs crate:catalog root:catalog
pub struct External;

//- /app/src/lib.rs crate:app root:app deps:catalog
fn load(_: catalog::External$0) {} // code_actions("external path").in_lib("app")
"#,
        expect![[r#"
            // external path
            refactor.rewrite Replace qualified path with `use`
              preferred: false
              result:
                use catalog::External;

                fn load(_: External) {}
        "#]],
    );
}

#[test]
fn qualified_path_supports_std_roots() {
    check_analysis_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:analysis_std_qualified_path_actions
fn load(_: std::sync::Arc$0<u8>) {} // code_actions("std path").in_lib("analysis_std_qualified_path_actions")
"#,
        expect![[r#"
            // std path
            refactor.rewrite Replace qualified path with `use`
              preferred: false
              result:
                use alloc::sync::Arc;

                fn load(_: Arc<u8>) {}
        "#]],
    );
}

#[test]
fn orders_independent_actions_stably_at_the_same_position() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_combined_code_actions
trait Service {
    fn required(&self);
}

struct Worker;
mod models { pub struct User; }

impl Service for Worker {
    fn helper(_: User$0) {} // code_actions("combined actions")
}
"#,
        expect![[r#"
            // combined actions
            quickfix Implement missing trait members
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

            quickfix Import `crate::models::User`
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
