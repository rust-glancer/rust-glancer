use expect_test::expect;

use crate::tests::utils::{check_analysis, check_analysis_with_fake_sysroot};
#[test]
#[ignore = "TODO(#160)"]
fn completes_unqualified_names_with_bounded_auto_imports() {
    check_analysis(
        r#"
//- /catalog/src/lib.rs crate:catalog root:catalog
pub mod collections {
    pub struct BTreeMap;
    pub struct HashMap;
}

pub mod left {
    pub struct Widget;
}

pub mod right {
    pub struct Widget;
}

mod internal {
    #[doc(hidden)]
    pub struct Reexported;
}

pub use internal::Reexported as PublicWidget;

pub mod facade {
    pub struct RankedWidget;
}
pub use facade::RankedWidget;

pub struct VisibleTarget;
#[doc(hidden)]
pub use VisibleTarget as SecretWidget;

#[doc(hidden)]
pub struct HiddenWidget;

#[unstable(feature = "internal", issue = "none")]
pub struct UnstableWidget;

#[cfg(any())]
pub struct DisabledWidget;

pub fn make_widget() {}

//- /app/src/lib.rs crate:app root:app deps:catalog
use catalog::collections::BTreeMap;

fn types() {
    let _: HashM$0; // complete_verbose_with_source("coalesced HashMap import").in_lib("app").matching("HashMap")
    let _: Wid$0; // complete_verbose_with_source("ambiguous imports").in_lib("app").matching("Widget")
    let _: RankedW$0; // complete_verbose_with_source("short re-export path").in_lib("app").matching("RankedWidget")
    let _: PublicW$0; // complete_verbose_with_source("public re-export").in_lib("app").matching("PublicWidget")
    let _: SecretW$0; // complete_with_source("hidden re-export").in_lib("app").matching("SecretWidget")
    let _: HiddenW$0; // complete_with_source("hidden declaration").in_lib("app").matching("HiddenWidget")
    let _: UnstableW$0; // complete_with_source("unstable declaration").in_lib("app").matching("UnstableWidget")
    let _: DisabledW$0; // complete_with_source("cfg-disabled declaration").in_lib("app").matching("DisabledWidget")
}

fn values() {
    make_w$0; // complete_verbose_with_source("value auto-import").in_lib("app").matching("make_widget")
}
"#,
        expect![[r#"
            // coalesced HashMap import
            struct HashMap
              detail: struct HashMap (use catalog::collections::HashMap)
              sort: 05-auto-import:0003|00|HashMap|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 61..66
              additional: 4..34 => "catalog::collections::{BTreeMap, HashMap}"

            // ambiguous imports
            struct Widget
              detail: struct Widget (use catalog::left::Widget)
              sort: 05-auto-import:0003|00|Widget|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(2) } })
              replace: 79..82
              additional: 35..35 => "\nuse catalog::left::Widget;"
            struct Widget
              detail: struct Widget (use catalog::right::Widget)
              sort: 05-auto-import:0003|00|Widget|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(3) } })
              replace: 79..82
              additional: 35..35 => "\nuse catalog::right::Widget;"

            // short re-export path
            struct RankedWidget
              detail: struct RankedWidget (use catalog::RankedWidget)
              sort: 05-auto-import:0002|00|RankedWidget|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(5) } })
              replace: 95..102
              additional: 35..35 => "\nuse catalog::RankedWidget;"

            // public re-export
            struct PublicWidget
              detail: struct PublicWidget (use catalog::PublicWidget)
              sort: 05-auto-import:0002|00|PublicWidget|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(4) } })
              replace: 115..122
              additional: 35..35 => "\nuse catalog::PublicWidget;"

            <none> // hidden re-export
            <none> // hidden declaration
            <none> // unstable declaration
            <none> // cfg-disabled declaration

            // value auto-import
            fn make_widget
              detail: pub fn make_widget() (use catalog::make_widget)
              sort: 05-auto-import:0002|make_widget|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), id: FunctionId(0) })
              replace: 229..235
              additional: 35..35 => "\nuse catalog::make_widget;"
              snippet: make_widget()$0
        "#]],
    );
}

#[test]
fn completes_body_local_imported_items() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_local_import_completions
pub fn use_it() {
    mod local {
        pub struct User;
        pub const VALUE: User = missing();
    }

    use local::*;

    let _typed: U$0; // complete("body-local imported type completions")
    let _value = V$0; // complete("body-local imported value completions")
}
"#,
        expect![[r#"
            // body-local imported type completions
            struct User
            module local

            // body-local imported value completions
            struct User
            const VALUE
            variable _typed
            module local
            fn use_it
        "#]],
    );
}

#[test]
fn completes_unqualified_import_roots_and_external_roots() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_unqualified_roots
pub struct Api;

//- /src/main.rs crate:analysis_unqualified_roots kind:bin name:analysis-unqualified-roots
use analysis_unqualified_roots$0; // complete("unqualified use root completions").in_bin("analysis_unqualified_roots")

fn main() {
    let _ = analysis_unqualified_roots$0; // complete("unqualified value root completions").in_bin("analysis_unqualified_roots")
}
"#,
        expect![[r#"
            // unqualified use root completions
            module analysis_unqualified_roots
            fn main

            // unqualified value root completions
            module analysis_unqualified_roots
            fn main
        "#]],
    );
}

#[test]
fn completes_absolute_keyword_rooted_paths_and_root_keywords() {
    check_analysis_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:analysis_rooted_path_completions
pub mod parent {
    pub mod sibling {
        pub struct Sibling;
    }

    pub mod child {
        use ::st$0; // complete("absolute root completions").in_lib("analysis_rooted_path_completions")
        use crate::par$0; // complete("crate root completions").in_lib("analysis_rooted_path_completions")
        use super::sib$0; // complete("super root completions").in_lib("analysis_rooted_path_completions")

        pub fn body() {}
    }
}

"#,
        expect![[r#"
            // absolute root completions
            module alloc
            module core
            module std

            module parent // crate root completions

            // super root completions
            module child
            module sibling
        "#]],
    );
}

#[test]
fn completes_unqualified_prelude_names() {
    check_analysis_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:analysis_unqualified_prelude_completions
pub fn use_it() {
    let _value: Vec$0; // complete("unqualified prelude completions").in_lib("analysis_unqualified_prelude_completions")
}
"#,
        expect![[r#"
            // unqualified prelude completions
            trait Clone
            trait Copy
            trait Default
            trait Eq
            trait Fn
            trait FnMut
            trait FnOnce
            trait IntoIterator
            trait Iterator
            enum Option
            trait Ord
            trait PartialEq
            trait PartialOrd
            enum Result
            trait Send
            struct String
            trait Sync
            struct Vec
            module alloc
            module core
            module std
        "#]],
    );
}

#[test]
fn completes_qualified_paths_in_use_items() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_use_path_completions
use crate::api::Ap$0; // complete("use path completions")

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
        expect![[r#"
            // use path completions
            type_alias ApiAlias
            trait ApiNamed
            enum ApiState
            struct ApiUser
            static FLAG
            const VERSION
            module api_nested
            fn build_user
        "#]],
    );
}

#[test]
fn completes_qualified_paths_inside_braced_use_items() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_braced_use_path_completions
use crate::api::{Ap$0}; // complete("braced use path completions")

pub mod api {
    pub mod api_nested {}
    pub struct ApiUser;
}
"#,
        expect![[r#"
            // braced use path completions
            struct ApiUser
            module api_nested
        "#]],
    );
}

#[test]
fn completes_qualified_paths_at_bare_use_path_coloncolon() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_bare_use_path_completions
use crate::api_notify::$0; // complete("bare use path completions")

pub mod api_notify {
    pub struct Notification;
}
"#,
        expect![[r#"
            struct Notification // bare use path completions
        "#]],
    );
}

#[test]
fn completes_qualified_paths_at_incomplete_bare_use_path_coloncolon() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_incomplete_bare_use_path_completions
pub mod api_notify {
    pub struct Notification;
}

use crate::api_notify::$0 // complete("incomplete bare use path completions")
"#,
        expect![[r#"
            struct Notification // incomplete bare use path completions
        "#]],
    );
}

#[test]
fn completes_sysroot_paths_at_incomplete_bare_use_path_coloncolon() {
    check_analysis_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:analysis_sysroot_bare_use_path_completions
use std::sync::$0 // complete("incomplete sysroot use path completions").in_lib("analysis_sysroot_bare_use_path_completions")

#[derive(Debug)]
enum CliInvocation {
    Capture(Vec<String>),
}

const DEFAULT_BASE_BRANCH: &str = "main";

pub fn run() {}
"#,
        expect![[r#"
            struct Arc // incomplete sysroot use path completions
        "#]],
    );
}
