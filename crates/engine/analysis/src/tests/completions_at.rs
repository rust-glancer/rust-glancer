use expect_test::expect;

use super::utils::{AnalysisQuery, check_analysis_queries, check_analysis_queries_with_sysroot};

#[test]
fn completes_inherent_and_trait_methods_at_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Named {
    fn trait_name(&self);
    fn associated() {}
}

pub struct User;

impl User {
    pub fn new() -> Self {
        User
    }

    pub fn id(&self) {}

    pub fn touch(&mut self) {}
}

impl Named for User {
    fn trait_name(&self) {}
}

pub fn use_it(user: User) {
    user.$0id();
}
"#,
        &[AnalysisQuery::complete("dot completions", "0")],
        expect![[r#"
            dot completions
            - inherent_method id
            - inherent_method touch
            - trait_method trait_name
        "#]],
    );
}

#[test]
fn completes_methods_at_bare_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_bare_dot_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Named {
    fn trait_name(&self);
}

pub struct User;

impl User {
    pub fn id(&self) {}

    pub fn touch(&mut self) {}
}

impl Named for User {
    fn trait_name(&self) {}
}

pub fn use_it(user: User) {
    user.$0;
}
"#,
        &[AnalysisQuery::complete("bare dot completions", "0")],
        expect![[r#"
            bare dot completions
            - inherent_method id
            - inherent_method touch
            - trait_method trait_name
        "#]],
    );
}

#[test]
fn completes_bare_dot_before_following_statement() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_bare_dot_before_statement_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User {
    pub name: String,
}

impl User {
    pub fn id(&self) {}
}

pub fn use_it(user: User) {
    user.$0

    user.id();
}
"#,
        &[AnalysisQuery::complete_verbose(
            "bare dot before statement completions",
            "0",
        )],
        expect![[r#"
            bare dot before statement completions
            - inherent_method id
              detail: pub fn id(&self)
              sort: id|01|00|Function(Semantic(FunctionRef { target: TargetRef { package: PackageSlot(0), target: TargetId(0) }, id: FunctionId(1) }))
              replace: 119..119
              snippet: id()$0
            - field name
              detail: pub name: String
              sort: name|00|00|Field(Semantic(FieldRef { owner: TypeDefRef { target: TargetRef { package: PackageSlot(0), target: TargetId(0) }, id: Struct(StructId(0)) }, index: 0 }))
              replace: 119..119
        "#]],
    );
}

#[test]
fn completes_qualified_module_paths_in_body_contexts() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod api {
    pub mod api_nested {}
    mod private_nested {}

    pub struct ApiUser;
    pub enum ApiState {}
    pub trait ApiNamed {}
    pub type ApiAlias = ApiUser;

    pub const VERSION: u8 = 1;
    pub static FLAG: bool = true;
    pub fn build_user() -> ApiUser {
        ApiUser
    }
}

pub fn use_it() {
    let _: crate::api::Ap$type_path$;
    let _ = 0 as crate::api::Ap$cast_type_path$;
    let _ = crate::api::bu$value_path$();
}
"#,
        &[
            AnalysisQuery::complete("type path completions", "type_path"),
            AnalysisQuery::complete("cast type path completions", "cast_type_path"),
            AnalysisQuery::complete("value path completions", "value_path"),
        ],
        // Value-position paths include type-namespace entries too because modules and nominal
        // types can be intermediate prefixes. Prefix filtering is left to the LSP client.
        expect![[r#"
            type path completions
            - type_alias ApiAlias
            - trait ApiNamed
            - enum ApiState
            - struct ApiUser
            - module api_nested

            cast type path completions
            - type_alias ApiAlias
            - trait ApiNamed
            - enum ApiState
            - struct ApiUser
            - module api_nested

            value path completions
            - type_alias ApiAlias
            - trait ApiNamed
            - enum ApiState
            - struct ApiUser
            - static FLAG
            - const VERSION
            - module api_nested
            - fn build_user
        "#]],
    );
}

#[test]
fn completes_bare_qualified_paths_in_value_contexts() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_bare_value_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod api {
    pub fn build_user() {}
}

pub fn make_root() {}

pub fn use_it() {
    let _foo = crate::$0
}
"#,
        &[AnalysisQuery::complete("bare value path completions", "0")],
        expect![[r#"
            bare value path completions
            - module api
            - fn make_root
            - fn use_it
        "#]],
    );
}

#[test]
fn completes_record_constructor_paths() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_record_constructor_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod api {
    pub struct User {
        pub id: u8,
    }

    pub enum Action {
        Start { id: u8 },
    }

    pub fn build_user() -> User {
        User { id: 0 }
    }
}

pub struct LocalUser {
    id: u8,
}

pub fn use_it() {
    enum LocalAction {
        Start { id: u8 },
    }

    let _local = Local$local_ctor$ { id: 0 };
    let _local_variant = LocalAction::Sta$local_variant_ctor$ { id: 0 };
    let _record = api::Us$record_ctor$ { id: 0 };
    let _variant = api::Action::Sta$variant_ctor$ { id: 0 };
}
"#,
        &[
            AnalysisQuery::complete("unqualified record constructor completions", "local_ctor"),
            AnalysisQuery::complete(
                "body-local record variant constructor completions",
                "local_variant_ctor",
            ),
            AnalysisQuery::complete("qualified record constructor completions", "record_ctor"),
            AnalysisQuery::complete("record variant constructor completions", "variant_ctor"),
        ],
        expect![[r#"
            unqualified record constructor completions
            - struct LocalUser
            - module api
            - fn use_it

            body-local record variant constructor completions
            - variant Start

            qualified record constructor completions
            - enum Action
            - struct User
            - fn build_user

            record variant constructor completions
            - variant Start
        "#]],
    );
}

#[test]
fn completes_bare_qualified_paths_in_type_contexts_without_semicolon() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_bare_type_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod api {
    pub struct User;
}

pub struct RootType;

pub fn use_it() {
    let _foo: crate::$0
}
"#,
        &[AnalysisQuery::complete("bare type path completions", "0")],
        expect![[r#"
            bare type path completions
            - struct RootType
            - module api
        "#]],
    );
}

#[test]
fn completes_qualified_paths_in_control_flow_patterns() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_control_flow_pattern_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod api {
    pub struct Profile(pub u8);
    pub struct User(pub u8);
    pub fn build() {}
}

pub struct Users;

pub fn use_it(user: api::User, users: Users) {
    if let api::Us$if_path$er(id) = user {}

    while let api::Us$while_path$er(id) = user {}

    for api::Us$for_path$er(id) in users {}
}
"#,
        &[
            AnalysisQuery::complete("if let pattern path completions", "if_path"),
            AnalysisQuery::complete("while let pattern path completions", "while_path"),
            AnalysisQuery::complete("for pattern path completions", "for_path"),
        ],
        expect![[r#"
            if let pattern path completions
            - struct Profile
            - struct User
            - fn build

            while let pattern path completions
            - struct Profile
            - struct User
            - fn build

            for pattern path completions
            - struct Profile
            - struct User
            - fn build
        "#]],
    );
}

#[test]
fn completes_unqualified_values_from_lexical_and_module_scope() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_value_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn make_user() -> User {
    User
}

pub fn use_it(input_user: User) {
    let local_user = input_user;
    let _selected = inp$0;
    let later_user = local_user;
}
"#,
        &[AnalysisQuery::complete(
            "unqualified value completions",
            "0",
        )],
        expect![[r#"
            unqualified value completions
            - struct User
            - variable input_user
            - variable local_user
            - fn make_user
            - fn use_it
        "#]],
    );
}

#[test]
fn unqualified_local_values_shadow_module_values() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_shadow_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn shadowed() {}

pub fn use_it(shadowed: u8) {
    let _ = sha$0;
}
"#,
        &[AnalysisQuery::complete("shadowed value completions", "0")],
        expect![[r#"
            shadowed value completions
            - variable shadowed
            - fn use_it
        "#]],
    );
}

#[test]
fn sorts_unqualified_body_values_by_lexical_proximity() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_value_proximity"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn c_module_item() {}

pub fn use_it(c_a_outer: u8) {
    {
        let c_z_inner = c_a_outer;
        c$0;
    }
}
"#,
        &[AnalysisQuery::complete_verbose(
            "unqualified value proximity",
            "0",
        )],
        expect![[r#"
            unqualified value proximity
            - variable c_z_inner
              detail: let c_z_inner: u8
              sort: 00-body:0000|c_z_inner|07|00|Binding { body: BodyRef { target: TargetRef { package: PackageSlot(0), target: TargetId(0) }, body: BodyId(1) }, binding: BindingId(1) }
              replace: 107..108
            - variable c_a_outer
              detail: let c_a_outer: u8
              sort: 00-body:0002|c_a_outer|07|00|Binding { body: BodyRef { target: TargetRef { package: PackageSlot(0), target: TargetId(0) }, body: BodyId(1) }, binding: BindingId(0) }
              replace: 107..108
            - fn c_module_item
              detail: pub fn c_module_item()
              sort: 01-module|c_module_item|06|00|Function(Semantic(FunctionRef { target: TargetRef { package: PackageSlot(0), target: TargetId(0) }, id: FunctionId(0) }))
              replace: 107..108
              snippet: c_module_item()$0
            - fn use_it
              detail: pub fn use_it(c_a_outer: u8)
              sort: 01-module|use_it|06|00|Function(Semantic(FunctionRef { target: TargetRef { package: PackageSlot(0), target: TargetId(0) }, id: FunctionId(1) }))
              replace: 107..108
              snippet: use_it(${1:c_a_outer})$0
        "#]],
    );
}

#[test]
fn completes_for_loop_pattern_bindings() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_for_pattern_binding_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Items;

pub fn use_it(items: Items) {
    for item in items {
        it$0;
    }
}
"#,
        &[AnalysisQuery::complete(
            "for pattern binding completions",
            "0",
        )],
        expect![[r#"
            for pattern binding completions
            - struct Items
            - variable item
            - variable items
            - fn use_it
        "#]],
    );
}

#[test]
fn sorts_unqualified_body_types_by_lexical_proximity() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_type_proximity"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod a_mod {}
pub trait ATrait {}
pub struct AModule;

pub fn use_it() {
    struct ZLocal;

    let _value: A$0;
}
"#,
        &[AnalysisQuery::complete_verbose(
            "unqualified type proximity",
            "0",
        )],
        expect![[r#"
            unqualified type proximity
            - struct ZLocal
              detail: struct ZLocal
              sort: 00-body:0000|00|ZLocal|00|BodyItem(BodyItemRef { body: BodyRef { target: TargetRef { package: PackageSlot(0), target: TargetId(0) }, body: BodyId(0) }, item: BodyItemId(0) })
              replace: 112..113
            - struct AModule
              detail: struct AModule
              sort: 01-module|00|AModule|00|Def(Local(LocalDefRef { target: TargetRef { package: PackageSlot(0), target: TargetId(0) }, local_def: LocalDefId(1) }))
              replace: 112..113
            - trait ATrait
              detail: trait ATrait
              sort: 01-module|01|ATrait|00|Def(Local(LocalDefRef { target: TargetRef { package: PackageSlot(0), target: TargetId(0) }, local_def: LocalDefId(0) }))
              replace: 112..113
            - module a_mod
              detail: mod a_mod
              sort: 01-module|02|a_mod|00|Def(Module(ModuleRef { target: TargetRef { package: PackageSlot(0), target: TargetId(0) }, module: ModuleId(1) }))
              replace: 112..113
        "#]],
    );
}

#[test]
fn completes_unqualified_types_from_body_and_module_scope() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_type_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct ModuleUser;

pub fn use_it() {
    struct LocalUser {
        id: u8,
    }

    let _value: Lo$0;
}
"#,
        &[AnalysisQuery::complete("unqualified type completions", "0")],
        expect![[r#"
            unqualified type completions
            - struct LocalUser
            - struct ModuleUser
        "#]],
    );
}

#[test]
fn completes_more_body_local_type_and_value_items() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_more_body_local_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct LocalUnit;
    enum LocalAction { Start }
    union LocalBits { id: GlobalId }
    type LocalAlias = GlobalId;
    trait LocalNamed {}
    const local_default: LocalAlias = GlobalId;
    static local_current: GlobalId = GlobalId;
    fn local_helper() -> LocalAlias {
        GlobalId
    }

    let _typed: Loc$type$;
    let _value = loc$value$;
}
"#,
        &[
            AnalysisQuery::complete("body-local type item completions", "type"),
            AnalysisQuery::complete("body-local value item completions", "value"),
        ],
        expect![[r#"
            body-local type item completions
            - struct GlobalId
            - enum LocalAction
            - type_alias LocalAlias
            - union LocalBits
            - trait LocalNamed
            - struct LocalUnit

            body-local value item completions
            - struct GlobalId
            - struct LocalUnit
            - variable _typed
            - static local_current
            - const local_default
            - fn local_helper
            - fn use_it
        "#]],
    );
}

#[test]
fn completes_unqualified_type_args_in_generic_type_paths() {
    check_analysis_queries_with_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_generic_type_arg_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Session;
pub struct State;
pub struct String;

pub enum Maybe<T> {
    Some(T),
    None,
}

pub fn use_it() {
    let _values: std::collections::HashMap<String, S$value_arg$>;
    let _variant = Maybe::<S$value_path_arg$>::None;
    let _keys: std::collections::HashMap<S$key_arg$
}

//- /sysroot/library/core/src/lib.rs
pub struct Core;

//- /sysroot/library/alloc/src/lib.rs
pub struct Alloc;

//- /sysroot/library/std/src/lib.rs
pub mod collections {
    pub struct HashMap<K, V>;
}
"#,
        &[
            AnalysisQuery::complete("first generic arg completions", "key_arg")
                .in_lib("analysis_unqualified_generic_type_arg_completions"),
            AnalysisQuery::complete("second generic arg completions", "value_arg")
                .in_lib("analysis_unqualified_generic_type_arg_completions"),
            AnalysisQuery::complete("value path generic arg completions", "value_path_arg")
                .in_lib("analysis_unqualified_generic_type_arg_completions"),
        ],
        expect![[r#"
            first generic arg completions
            - enum Maybe
            - struct Session
            - struct State
            - struct String
            - module alloc
            - module core
            - module std

            second generic arg completions
            - enum Maybe
            - struct Session
            - struct State
            - struct String
            - module alloc
            - module core
            - module std

            value path generic arg completions
            - enum Maybe
            - struct Session
            - struct State
            - struct String
            - module alloc
            - module core
            - module std
        "#]],
    );
}

#[test]
fn sorts_unqualified_type_context_completions_by_type_likelihood() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_type_sorting"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod aa_prefix {}
pub trait Mid {}
pub struct Zed;

pub fn use_it() {
    let _value: Z$0;
}
"#,
        &[AnalysisQuery::complete_verbose(
            "unqualified type sorting",
            "0",
        )],
        expect![[r#"
            unqualified type sorting
            - struct Zed
              detail: struct Zed
              sort: 01-module|00|Zed|00|Def(Local(LocalDefRef { target: TargetRef { package: PackageSlot(0), target: TargetId(0) }, local_def: LocalDefId(1) }))
              replace: 89..90
            - trait Mid
              detail: trait Mid
              sort: 01-module|01|Mid|00|Def(Local(LocalDefRef { target: TargetRef { package: PackageSlot(0), target: TargetId(0) }, local_def: LocalDefId(0) }))
              replace: 89..90
            - module aa_prefix
              detail: mod aa_prefix
              sort: 01-module|02|aa_prefix|00|Def(Module(ModuleRef { target: TargetRef { package: PackageSlot(0), target: TargetId(0) }, module: ModuleId(1) }))
              replace: 89..90
        "#]],
    );
}

#[test]
fn completes_unqualified_import_roots_and_external_roots() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_roots"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[[bin]]
name = "analysis-unqualified-roots"
path = "src/main.rs"

//- /src/lib.rs
pub struct Api;

//- /src/main.rs
use analysis_unqualified_roots$use_root$;

fn main() {
    let _ = analysis_unqualified_roots$value_root$;
}
"#,
        &[
            AnalysisQuery::complete("unqualified use root completions", "use_root")
                .in_bin("analysis_unqualified_roots"),
            AnalysisQuery::complete("unqualified value root completions", "value_root")
                .in_bin("analysis_unqualified_roots"),
        ],
        expect![[r#"
            unqualified use root completions
            - module analysis_unqualified_roots
            - fn main

            unqualified value root completions
            - module analysis_unqualified_roots
            - fn main
        "#]],
    );
}

#[test]
fn completes_unqualified_prelude_names() {
    check_analysis_queries_with_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_prelude_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    let _value: Vec$0;
}

//- /sysroot/library/core/src/lib.rs
pub struct Core;

//- /sysroot/library/alloc/src/lib.rs
pub mod vec {
    pub struct Vec;
}

//- /sysroot/library/std/src/lib.rs
pub mod prelude {
    pub mod rust_2024 {
        pub use alloc::vec::Vec;
    }
}
"#,
        &[
            AnalysisQuery::complete("unqualified prelude completions", "0")
                .in_lib("analysis_unqualified_prelude_completions"),
        ],
        expect![[r#"
            unqualified prelude completions
            - struct Vec
            - module alloc
            - module core
            - module std
        "#]],
    );
}

#[test]
fn completes_qualified_paths_in_use_items() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_use_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
use crate::api::Ap$use_path$;

pub mod api {
    pub mod api_nested {}
    mod private_nested {}

    pub struct ApiUser;
    pub enum ApiState {}
    pub trait ApiNamed {}
    pub type ApiAlias = ApiUser;

    pub const VERSION: u8 = 1;
    pub static FLAG: bool = true;
    pub fn build_user() -> ApiUser {
        ApiUser
    }
}
"#,
        &[AnalysisQuery::complete("use path completions", "use_path")],
        expect![[r#"
            use path completions
            - type_alias ApiAlias
            - trait ApiNamed
            - enum ApiState
            - struct ApiUser
            - static FLAG
            - const VERSION
            - module api_nested
            - fn build_user
        "#]],
    );
}

#[test]
fn completes_qualified_paths_inside_braced_use_items() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_braced_use_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
use crate::api::{Ap$use_path$};

pub mod api {
    pub mod api_nested {}
    pub struct ApiUser;
}
"#,
        &[AnalysisQuery::complete(
            "braced use path completions",
            "use_path",
        )],
        expect![[r#"
            braced use path completions
            - struct ApiUser
            - module api_nested
        "#]],
    );
}

#[test]
fn completes_qualified_paths_at_bare_use_path_coloncolon() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_bare_use_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
use crate::api_notify::$0;

pub mod api_notify {
    pub struct Notification;
}
"#,
        &[AnalysisQuery::complete("bare use path completions", "0")],
        expect![[r#"
            bare use path completions
            - struct Notification
        "#]],
    );
}

#[test]
fn completes_qualified_paths_at_incomplete_bare_use_path_coloncolon() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_incomplete_bare_use_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod api_notify {
    pub struct Notification;
}

use crate::api_notify::$0
"#,
        &[AnalysisQuery::complete(
            "incomplete bare use path completions",
            "0",
        )],
        expect![[r#"
            incomplete bare use path completions
            - struct Notification
        "#]],
    );
}

#[test]
fn completes_sysroot_paths_at_incomplete_bare_use_path_coloncolon() {
    check_analysis_queries_with_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_sysroot_bare_use_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
use std::env;
use std::ffi::OsString;

use std::collections::$0

#[derive(Debug)]
enum CliInvocation {
    Capture(Vec<String>),
}

const DEFAULT_BASE_BRANCH: &str = "main";

pub fn run() {}

//- /sysroot/library/core/src/lib.rs
pub struct Core;

//- /sysroot/library/alloc/src/lib.rs
pub mod collections {
    pub struct HashMap;
    pub struct HashSet;
}

//- /sysroot/library/std/src/lib.rs
pub use alloc::collections;
"#,
        &[
            AnalysisQuery::complete("incomplete sysroot use path completions", "0")
                .in_lib("analysis_sysroot_bare_use_path_completions"),
        ],
        expect![[r#"
            incomplete sysroot use path completions
            - struct HashMap
            - struct HashSet
        "#]],
    );
}

#[test]
fn completes_qualified_paths_with_replacement_range() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_path_completion_metadata"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod api {
    pub struct User;
}

pub fn use_it() {
    let _: crate::api::Us$0;
}
"#,
        &[AnalysisQuery::complete_verbose(
            "path metadata completions",
            "0",
        )],
        expect![[r#"
            path metadata completions
            - struct User
              detail: struct User
              sort: User|04|00|Def(Local(LocalDefRef { target: TargetRef { package: PackageSlot(0), target: TargetId(0) }, local_def: LocalDefId(0) }))
              replace: 79..81
        "#]],
    );
}

#[test]
fn completes_through_references_try_and_await_wrappers() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_wrapper_completion"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Result<T, E> {
    Ok(T),
    Err(E),
}

pub struct Error;

pub struct User {
    profile: Profile,
}

impl User {
    pub fn id(&self) {}
}

pub struct Profile;

pub fn load_user() -> Result<User, Error> {
    todo!()
}

pub async fn load_user_async() -> User {
    User { profile: Profile }
}

pub async fn use_it(user: User) -> Result<(), Error> {
    let raw = 0;
    (&user).$reference$;
    load_user()?.$try$;
    load_user_async().await.$await$;
    (raw as User).$cast$;
    Result::Ok(())
}
"#,
        &[
            AnalysisQuery::complete("reference completions", "reference"),
            AnalysisQuery::complete("try completions", "try"),
            AnalysisQuery::complete("await completions", "await"),
            AnalysisQuery::complete("cast completions", "cast"),
        ],
        expect![[r#"
            reference completions
            - inherent_method id
            - field profile

            try completions
            - inherent_method id
            - field profile

            await completions
            - inherent_method id
            - field profile

            cast completions
            - inherent_method id
            - field profile
        "#]],
    );
}

#[test]
fn completes_methods_for_bin_root_library_type() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_bin_completion"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[[bin]]
name = "analysis-bin-completion"
path = "src/main.rs"

//- /src/lib.rs
pub struct Api;

impl Api {
    pub fn ping(&self) {}
    pub fn work(&self) {}
}

//- /src/main.rs
fn main() {
    let api: analysis_bin_completion::Api = todo!();
    api.$0;
}
"#,
        &[AnalysisQuery::complete("bin root completions", "0").in_bin("analysis_bin_completion")],
        expect![[r#"
            bin root completions
            - inherent_method ping
            - inherent_method work
        "#]],
    );
}

#[test]
fn does_not_trigger_inside_method_arguments() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_dot_range"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

impl User {
    pub fn id(&self, _value: u8) {}

    pub fn touch(&self) {}
}

pub fn use_it(user: User) {
    user.id($inside_arg$0);
}
"#,
        &[AnalysisQuery::complete(
            "completion inside method argument",
            "inside_arg",
        )],
        expect![[r#"
            completion inside method argument
            - <none>
        "#]],
    );
}

#[test]
fn preserves_distinct_same_name_candidates() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_duplicates"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Named {
    fn label(&self);
}

pub trait Displayed {
    fn label(&self);
}

pub struct User;

impl User {
    pub fn label(&self) {}
}

impl Named for User {
    fn label(&self) {}
}

impl Displayed for User {
    fn label(&self) {}
}

pub fn use_it(user: User) {
    user.$0label();
}
"#,
        &[AnalysisQuery::complete("same-name completions", "0")],
        expect![[r#"
            same-name completions
            - inherent_method label
            - trait_method label
            - trait_method label
        "#]],
    );
}

#[test]
fn does_not_complete_concrete_impl_methods_for_wrong_generic_args() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_concrete_impl_completion"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Error;

pub struct Wrapper<T> {
    value: T,
}

impl<T> Wrapper<T> {
    pub fn generic(&self) {}
}

impl Wrapper<User> {
    pub fn user_only(&self) {}
}

pub trait UserOnlyTrait {
    fn trait_user_only(&self);
}

impl UserOnlyTrait for Wrapper<User> {
    fn trait_user_only(&self) {}
}

pub fn use_it(error: Wrapper<Error>) {
    error.$0;
}
"#,
        &[AnalysisQuery::complete(
            "wrong generic arg completions",
            "0",
        )],
        expect![[r#"
            wrong generic arg completions
            - inherent_method generic
            - field value
        "#]],
    );
}

#[test]
fn marks_generic_trait_method_completions_as_maybe() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_generic_trait_completion"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub struct Wrapper<T> {
    value: T,
}

pub trait GenericNamed {
    fn generic_trait_name(&self);
}

impl<T> GenericNamed for Wrapper<T> {
    fn generic_trait_name(&self) {}
}

pub trait BoundNamed {
    fn bounded_trait_name(&self);
}

pub trait Required {}

impl<T> BoundNamed for Wrapper<T>
where
    T: Required,
{
    fn bounded_trait_name(&self) {}
}

pub fn use_it(wrapper: Wrapper<User>) {
    wrapper.$0;
}
"#,
        &[AnalysisQuery::complete("generic trait completions", "0")],
        expect![[r#"
            generic trait completions
            - trait_method bounded_trait_name (maybe)
            - trait_method generic_trait_name (maybe)
            - field value
        "#]],
    );
}

#[test]
fn completes_methods_after_field_receiver() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_field_receiver_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Profile;

impl Profile {
    pub fn display(&self) {}
}

pub struct User {
    pub profile: Profile,
}

pub fn use_it(user: User) {
    user.profile.$0;
}
"#,
        &[AnalysisQuery::complete("field receiver completions", "0")],
        expect![[r#"
            field receiver completions
            - inherent_method display
        "#]],
    );
}

#[test]
fn completes_fields_at_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_field_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Profile;

pub struct User {
    pub profile: Profile,
}

pub fn use_it(user: User) {
    user.$0;
}
"#,
        &[AnalysisQuery::complete("field completions", "0")],
        expect![[r#"
            field completions
            - field profile
        "#]],
    );
}

#[test]
fn completes_body_local_struct_fields_at_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_field_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn use_it() {
    struct User {
        id: UserId,
        profile: Profile,
    }
    struct Pair(UserId, Profile);
    struct UserId;
    struct Profile;

    let user: User;
    user.$0;

    let pair: Pair;
    pair.$tuple$;
}
"#,
        &[
            AnalysisQuery::complete("body-local field completions", "0"),
            AnalysisQuery::complete("body-local tuple field completions", "tuple"),
        ],
        expect![[r#"
            body-local field completions
            - field id
            - field profile

            body-local tuple field completions
            - field 0
            - field 1
        "#]],
    );
}

#[test]
fn completes_body_local_impl_methods_at_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_impl_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User {
        id: GlobalId,
    }

    impl User {
        fn id(&self) -> GlobalId {
            missing()
        }

        fn associated() -> GlobalId {
            missing()
        }
    }

    let user: User;
    user.$0;
}
"#,
        &[AnalysisQuery::complete("body-local impl completions", "0")],
        expect![[r#"
            body-local impl completions
            - field id
            - inherent_method id
        "#]],
    );
}

#[test]
fn completes_body_local_impl_methods_from_nested_blocks() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_nested_body_local_impl_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User {
        id: GlobalId,
    }

    {
        impl User {
            fn id(&self) -> GlobalId {
                missing()
            }
        }
    }

    let user: User;
    user.$0;
}
"#,
        &[AnalysisQuery::complete(
            "nested body-local impl completions",
            "0",
        )],
        expect![[r#"
            nested body-local impl completions
            - field id
            - inherent_method id
        "#]],
    );
}

#[test]
fn completes_body_local_generic_impl_method_return_and_field_receivers() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_generic_impl_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    struct Id;
    struct Error;
    struct User {
        id: Id,
    }

    impl User {
        fn label(&self) {}
    }

    struct Wrapper<T> {
        value: T,
    }

    impl<U> Wrapper<U> {
        fn get(&self) -> U {
            missing()
        }
    }

    impl Wrapper<User> {
        fn user_only(&self) -> User {
            missing()
        }
    }

    let wrapper: Wrapper<User>;
    wrapper.get().$method_return$;
    wrapper.value.$field_receiver$;

    let error: Wrapper<Error>;
    error.$wrong_receiver$;
}
"#,
        &[
            AnalysisQuery::complete("generic method return completions", "method_return"),
            AnalysisQuery::complete("generic field receiver completions", "field_receiver"),
            AnalysisQuery::complete("wrong generic receiver completions", "wrong_receiver"),
        ],
        expect![[r#"
            generic method return completions
            - field id
            - inherent_method label

            generic field receiver completions
            - field id
            - inherent_method label

            wrong generic receiver completions
            - inherent_method get
            - field value
        "#]],
    );
}

#[test]
fn completes_fields_and_methods_after_enum_pattern_payloads() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_enum_pattern_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Id;

pub struct User {
    id: Id,
}

impl User {
    fn is_valid(&self) -> bool {
        true
    }

    fn label(&self) {}
}

pub enum Option<T> {
    Some(T),
    None,
}

pub fn use_it(maybe: Option<User>) {
    let Some(value) = maybe else { return; };
    value.$let_payload$;

    if let Some(found) = maybe && found.$if_rhs$is_valid() {
        found.$if_payload$;
    }

    while let Some(next) = maybe {
        next.$while_payload$;
    }

    match maybe {
        Some(user) if user.$match_guard$is_valid() => user.$match_payload$,
        None => {}
    }
}
"#,
        &[
            AnalysisQuery::complete("let pattern payload completions", "let_payload"),
            AnalysisQuery::complete("if let-chain rhs completions", "if_rhs"),
            AnalysisQuery::complete("if let pattern payload completions", "if_payload"),
            AnalysisQuery::complete("while let pattern payload completions", "while_payload"),
            AnalysisQuery::complete("match guard payload completions", "match_guard"),
            AnalysisQuery::complete("match pattern payload completions", "match_payload"),
        ],
        expect![[r#"
            let pattern payload completions
            - field id
            - inherent_method is_valid
            - inherent_method label

            if let-chain rhs completions
            - field id
            - inherent_method is_valid
            - inherent_method label

            if let pattern payload completions
            - field id
            - inherent_method is_valid
            - inherent_method label

            while let pattern payload completions
            - field id
            - inherent_method is_valid
            - inherent_method label

            match guard payload completions
            - field id
            - inherent_method is_valid
            - inherent_method label

            match pattern payload completions
            - field id
            - inherent_method is_valid
            - inherent_method label
        "#]],
    );
}

#[test]
fn completes_fields_and_methods_after_closure_params() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_closure_pattern_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Id;

pub struct User {
    id: Id,
}

impl User {
    fn is_valid(&self) -> bool {
        true
    }

    fn label(&self) {}
}

pub fn use_it() {
    let _closure = |user: User| user.$closure_payload$;
}
"#,
        &[AnalysisQuery::complete(
            "closure param payload completions",
            "closure_payload",
        )],
        expect![[r#"
            closure param payload completions
            - field id
            - inherent_method is_valid
            - inherent_method label
        "#]],
    );
}

#[test]
fn completes_tuple_fields_at_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_tuple_field_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Left;
pub struct Right;

pub struct Pair(pub Left, pub Right);

pub fn use_it(pair: Pair) {
    pair.$0;
}
"#,
        &[AnalysisQuery::complete("tuple field completions", "0")],
        expect![[r#"
            tuple field completions
            - field 0
            - field 1
        "#]],
    );
}

#[test]
fn completes_dot_members_with_metadata_and_replacement_range() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_metadata"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Profile;

pub struct User {
    /// Name field.
    pub name: Profile,
}

impl User {
    /// Name method.
    pub fn name(&self) -> Profile {
        todo!()
    }
}

pub fn use_it(user: User) {
    user.na$0;
}
"#,
        &[AnalysisQuery::complete_verbose("metadata completions", "0")],
        expect![[r#"
            metadata completions
            - field name
              detail: pub name: Profile
              docs: Name field.
              sort: name|00|00|Field(Semantic(FieldRef { owner: TypeDefRef { target: TargetRef { package: PackageSlot(0), target: TargetId(0) }, id: Struct(StructId(1)) }, index: 0 }))
              replace: 216..218
            - inherent_method name
              detail: pub fn name(&self) -> Profile
              docs: Name method.
              sort: name|01|00|Function(Semantic(FunctionRef { target: TargetRef { package: PackageSlot(0), target: TargetId(0) }, id: FunctionId(1) }))
              replace: 216..218
              snippet: name()$0
        "#]],
    );
}

#[test]
fn completes_record_literal_fields() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_record_literal_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User {
    pub id: u8,
    pub name: u8,
    pub active: bool,
}

pub fn use_it(id: u8) {
    let _with_prefix = User { id, na$literal_prefix$ };
    let _empty = User { id, $literal_empty$ };
    let _defaults = User { ..$literal_defaults$ };
}
"#,
        &[
            AnalysisQuery::complete("record literal prefix completions", "literal_prefix"),
            AnalysisQuery::complete("record literal empty completions", "literal_empty"),
            AnalysisQuery::complete("record literal defaults completions", "literal_defaults"),
        ],
        expect![[r#"
            record literal prefix completions
            - field active
            - field name

            record literal empty completions
            - field active
            - field name

            record literal defaults completions
            - <none>
        "#]],
    );
}

#[test]
fn completes_record_pattern_fields() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_record_pattern_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User {
    pub id: u8,
    pub name: u8,
    pub active: bool,
}

pub fn use_it(user: User) {
    let User { id, na$pattern_prefix$ } = user;
    let User { ..$pattern_rest$ } = user;
}
"#,
        &[
            AnalysisQuery::complete("record pattern prefix completions", "pattern_prefix"),
            AnalysisQuery::complete("record pattern rest completions", "pattern_rest"),
        ],
        expect![[r#"
            record pattern prefix completions
            - field active
            - field name

            record pattern rest completions
            - <none>
        "#]],
    );
}

#[test]
fn completes_record_pattern_fields_in_control_flow() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_control_flow_record_pattern_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User {
    pub id: u8,
    pub name: u8,
    pub active: bool,
}

pub struct Users;

pub fn use_it(user: User, users: Users) {
    if let User { id, na$if_field$ } = user {}

    while let User { ac$while_field$ } = user {}

    for User { id, na$for_field$ } in users {}
}
"#,
        &[
            AnalysisQuery::complete("if let record pattern fields", "if_field"),
            AnalysisQuery::complete("while let record pattern fields", "while_field"),
            AnalysisQuery::complete("for record pattern fields", "for_field"),
        ],
        expect![[r#"
            if let record pattern fields
            - field active
            - field name

            while let record pattern fields
            - field active
            - field id
            - field name

            for record pattern fields
            - field active
            - field name
        "#]],
    );
}

#[test]
fn completes_body_local_record_literal_fields() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_record_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    struct Local {
        left: u8,
        right: u8,
    }

    let _value = Local { $0 };
}
"#,
        &[AnalysisQuery::complete(
            "body-local record literal completions",
            "0",
        )],
        expect![[r#"
            body-local record literal completions
            - field left
            - field right
        "#]],
    );
}

#[test]
fn keeps_record_field_value_positions_as_value_completions() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_record_value_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User {
    pub id: u8,
    pub name: u8,
}

pub fn use_it(user_value: u8) {
    let _value = User { id: us$0, name: 0 };
}
"#,
        &[AnalysisQuery::complete(
            "record field value completions",
            "0",
        )],
        expect![[r#"
            record field value completions
            - struct User
            - fn use_it
            - variable user_value
        "#]],
    );
}
