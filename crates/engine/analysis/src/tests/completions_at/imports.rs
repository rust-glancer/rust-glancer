use expect_test::expect;

use crate::tests::utils::{check_analysis, check_analysis_with_fake_sysroot};

#[test]
fn merged_reexports_keep_a_documented_spelling() {
    check_analysis(
        r#"
//- /catalog/src/lib.rs crate:catalog root:catalog
mod source {
    pub struct Shared {}
}

pub mod hidden_first {
    #[doc(hidden)]
    pub use crate::source::*;
    pub use crate::source::*;
}

pub mod visible_first {
    pub use crate::source::*;
    #[doc(hidden)]
    pub use crate::source::*;
}

pub mod macros {
    #[doc(hidden)]
    #[macro_export]
    macro_rules! facade_macro { () => {} }

    #[doc(inline)]
    pub use facade_macro;
}

//- /app/src/lib.rs crate:app root:app deps:catalog
fn use_it() {
    let _: catalog::hidden_first::Sha$0; // complete("visible re-export after hidden source").in_lib("app").matching("Shared")
    let _: catalog::visible_first::Sha$0; // complete("visible re-export before hidden source").in_lib("app").matching("Shared")
    catalog::macros::facade_$0!(); // complete("explicit inline macro re-export").in_lib("app").matching("facade_macro")
}
"#,
        expect![[r#"
            st Shared // visible re-export after hidden source
            st Shared // visible re-export before hidden source
            ma facade_macro // explicit inline macro re-export
        "#]],
    );
}

#[test]
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

#[doc(inline)]
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
    let _: HashM$0; // complete_verbose("coalesced HashMap import").in_lib("app").auto_import().matching("HashMap")
    let _: Wid$0; // complete_verbose("ambiguous imports").in_lib("app").auto_import().matching("Widget")
    let _: RankedW$0; // complete_verbose("short re-export path").in_lib("app").auto_import().matching("RankedWidget")
    let _: PublicW$0; // complete_verbose("public re-export").in_lib("app").auto_import().matching("PublicWidget")
    let _: SecretW$0; // complete("hidden re-export").in_lib("app").auto_import().matching("SecretWidget")
    let _: HiddenW$0; // complete("hidden declaration").in_lib("app").auto_import().matching("HiddenWidget")
    let _: UnstableW$0; // complete("unstable declaration").in_lib("app").auto_import().matching("UnstableWidget")
    let _: DisabledW$0; // complete("cfg-disabled declaration").in_lib("app").auto_import().matching("DisabledWidget")
}

fn values() {
    make_w$0; // complete_verbose("value auto-import").in_lib("app").auto_import().matching("make_widget")
}
"#,
        expect![[r#"
            // coalesced HashMap import
            struct HashMap
              detail: struct HashMap
              import: use catalog::collections::HashMap
              replace: 61..66
              additional: 4..34 => "catalog::collections::{BTreeMap, HashMap}"

            // ambiguous imports
            struct Widget
              detail: struct Widget
              import: use catalog::left::Widget
              replace: 79..82
              additional: 35..35 => "\nuse catalog::left::Widget;"
            struct Widget
              detail: struct Widget
              import: use catalog::right::Widget
              replace: 79..82
              additional: 35..35 => "\nuse catalog::right::Widget;"

            // short re-export path
            struct RankedWidget
              detail: struct RankedWidget
              import: use catalog::RankedWidget
              replace: 95..102
              additional: 35..35 => "\nuse catalog::RankedWidget;"

            // public re-export
            struct PublicWidget
              detail: struct PublicWidget
              import: use catalog::PublicWidget
              replace: 115..122
              additional: 35..35 => "\nuse catalog::PublicWidget;"

            <none> // hidden re-export
            <none> // hidden declaration
            <none> // unstable declaration
            <none> // cfg-disabled declaration

            // value auto-import
            fn make_widget
              detail: pub fn make_widget()
              import: use catalog::make_widget
              replace: 229..235
              additional: 35..35 => "\nuse catalog::make_widget;"
              snippet: make_widget()$0
        "#]],
    );
}

#[test]
fn offers_imports_that_are_usable_at_the_cursor() {
    check_analysis(
        r#"
//- /src/lib.rs crate:import_bindings
mod catalog {
    pub struct Widget;
    pub struct Record {}
    pub trait Paint {}
    pub trait Canvas {}
    pub trait Source { type Item; }
    pub trait Draw {
        fn draw_with<T: Paint>(&self, brush: T) where Self: Canvas {}
        fn draw_item<T: Paint>(&self) where Self: Source<Item = T> {}
        fn draw_from<T: Source>(&self, other: T) where Self: Source<Item = T::Item> {}
    }
    impl Draw for u32 {}
    impl Draw for bool {}
    impl Canvas for bool {}
    impl Paint for bool {}
    impl Source for bool { type Item = bool; }
    impl Source for u32 { type Item = u32; }
    pub struct Bundled {}
    pub use crate::constants::Bundled;
}
mod constants { pub const Bundled: u32 = 0; }
mod explicit {
    use crate::constants::Bundled;
    fn f() { let _: Bun$0; } // complete("explicit constant would be imported twice").auto_import().matching("Bundled")
}
mod glob {
    use crate::constants::*;
    fn f() { let _: Bun$0; } // complete("explicit use can shadow the same glob constant").auto_import().matching("Bundled")
}
fn Widget() {}
fn Record() {}
fn draw(yes: bool, no: u32) {
    let _: Wid$0; // complete("constructor conflicts with function").auto_import().matching("Widget")
    let _: Rec$0; // complete("record and function can share a name").auto_import().matching("Record")
    yes.draw_w$0; // complete("only the brush type is unknown").auto_import().matching("draw_with")
    no.draw_w$0; // complete("receiver is not a canvas").auto_import().matching("draw_with")
    yes.draw_i$0; // complete("receiver determines a paintable item").auto_import().matching("draw_item")
    no.draw_i$0; // complete("receiver determines an unpaintable item").auto_import().matching("draw_item")
    yes.draw_f$0; // complete_verbose("the other source is chosen by the caller").auto_import().matching("draw_from")
}
"#,
        expect![[r#"
            <none> // explicit constant would be imported twice
            st Bundled // explicit use can shadow the same glob constant
            <none> // constructor conflicts with function
            st Record // record and function can share a name
            tm draw_with // only the brush type is unknown
            <none> // receiver is not a canvas
            tm draw_item // receiver determines a paintable item
            <none> // receiver determines an unpaintable item

            // the other source is chosen by the caller
            trait_method draw_from
              detail: fn draw_from<T: Source>(&self, other: T) where Self: Source<Item = T::Item>
              import: use crate::catalog::Draw as _
              replace: 1020..1026
              additional: 0..0 => "use crate::catalog::Draw as _;\n\n"
              snippet: draw_from(${1:other})$0
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
            st User
            md local

            // body-local imported value completions
            vr _typed
            st User
            ct VALUE
            md local
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
            md analysis_unqualified_roots
            fn main

            // unqualified value root completions
            fn main
            md analysis_unqualified_roots
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
            md alloc
            md core
            md std

            md parent // crate root completions

            // super root completions
            md child
            md sibling
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
            en Option
            en Result
            st String
            st Vec
            tt Clone
            tt Copy
            tt Default
            tt Eq
            tt Fn
            tt FnMut
            tt FnOnce
            tt IntoIterator
            tt Iterator
            tt Ord
            tt PartialEq
            tt PartialOrd
            tt Send
            tt Sync
            md alloc
            md core
            md std
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
            ta ApiAlias
            tt ApiNamed
            en ApiState
            st ApiUser
            sc FLAG
            ct VERSION
            md api_nested
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
            st ApiUser
            md api_nested
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
            st Notification // bare use path completions
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
            st Notification // incomplete bare use path completions
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
            st Arc // incomplete sysroot use path completions
        "#]],
    );
}
