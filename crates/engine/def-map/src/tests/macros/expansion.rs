use expect_test::expect;
use rg_ir_model::PackageSlot;

use crate::{profile::metric, profile_descriptors, tests::utils};

#[test]
fn expands_local_macro_rules_items() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:macro_fixture
macro_rules! make_user {
    () => {
        pub struct User;
    };
}

make_user!();
"#,
    );
    let target = project.lib("macro_fixture");

    target
        .entry("User")
        .assert_type_exists("macro expansion should add generated structs to the module scope");
    assert!(
        project
            .def_map_db()
            .resident_package(PackageSlot(0))
            .expect("macro fixture package should remain resident")
            .macro_expansion_limits()
            .is_empty(),
        "successful expansion should not retain an empty limit diagnostic",
    );
}

#[test]
fn unsupported_proc_macro_calls_do_not_block_ordinary_items() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /macros/src/lib.rs crate:fixture_macros root:macros kind:proc-macro
extern crate proc_macro;

#[proc_macro]
pub fn emit(_input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    proc_macro::TokenStream::new()
}

//- /app/src/lib.rs crate:fixture_app root:app deps:fixture_macros
use fixture_macros::emit;

emit!();

pub struct AfterProcMacroCall;
"#,
    );
    let target = project.lib("fixture_app");

    target.entry("AfterProcMacroCall").assert_type_exists(
        "unsupported proc-macro execution should not abort collection of ordinary items",
    );
}

#[test]
fn generated_enum_variants_keep_shape_namespace_occupancy() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:generated_variant_fixture
macro_rules! make_choice {
    () => {
        pub enum Choice {
            Record { value: u8 },
            Tuple(u8),
            Unit,
        }
    };
}

make_choice!();
use Choice::{Record, Tuple, Unit};
"#,
    );
    let target = project.lib("generated_variant_fixture");

    target
        .entry("Record")
        .assert_type_exists("generated record variants should be importable as record paths")
        .assert_value_missing("generated record variants should not become bare values");
    for name in ["Tuple", "Unit"] {
        target
            .entry(name)
            .assert_type_exists("generated tuple and unit variants should retain type bindings")
            .assert_value_exists(
                "generated tuple and unit variants should retain value constructors",
            );
    }
}

#[test]
fn generated_impls_keep_generated_source_identity() {
    utils::check_project_def_map(
        r#"
//- /src/lib.rs crate:macro_impl_fixture
macro_rules! make_user {
    () => {
        pub struct User;

        impl User {
            pub fn new() -> Self {
                User
            }
        }
    };
}

make_user!();
"#,
        expect![[r#"
            package macro_impl_fixture

            macro_impl_fixture [lib]
            crate
            - User : type [pub struct macro_impl_fixture[lib]::crate::User] | value [pub struct macro_impl_fixture[lib]::crate::User]
            - make_user : macro [macro_definition macro_impl_fixture[lib]::crate::make_user]
            impls
            - impl generated#0:2
        "#]],
    );
}

#[test]
fn resolves_imports_generated_by_macros() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:macro_import_fixture
mod source {
    pub struct Thing;
}

macro_rules! import_thing {
    () => {
        pub use source::Thing;
    };
}

import_thing!();
"#,
    );
    let target = project.lib("macro_import_fixture");

    target
        .entry("Thing")
        .assert_type_exists("macro-generated imports should participate in import resolution");
}

#[test]
fn generated_declarations_replace_glob_bindings_through_reexports() {
    utils::check_project_path_resolution(
        r#"
//- /src/lib.rs crate:generated_binding_refresh
mod original {
    pub struct Item;
}

mod bridge {
    pub use crate::original::*;

    macro_rules! declare_item {
        () => { pub struct Item; };
    }

    declare_item!();
}

pub use bridge::Item;
"#,
        &[
            utils::PathResolutionQuery::lib("generated_binding_refresh", "crate", "bridge::Item")
                .types(),
            utils::PathResolutionQuery::lib("generated_binding_refresh", "crate", "Item").types(),
        ],
        expect![[r#"
            generated_binding_refresh [lib] crate resolves bridge::Item [types] -> struct generated_binding_refresh[lib]::crate::bridge::Item
            generated_binding_refresh [lib] crate resolves Item [types] -> struct generated_binding_refresh[lib]::crate::bridge::Item
        "#]],
    );
}

#[test]
fn resolves_dollar_crate_in_generated_imports_to_macro_definition_crate() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /crates/dep/src/lib.rs crate:dep root:crates/dep
pub mod source {
    pub struct Thing;
}

#[macro_export]
macro_rules! import_thing {
    () => {
        pub use $crate::source::Thing;
    };
}

//- /crates/app/src/lib.rs crate:app root:crates/app deps:dep
use dep::import_thing;

import_thing!();
"#,
    );
    let target = project.lib("app");

    target
        .entry("Thing")
        .assert_type_exists("$crate in dependency macros should resolve to the defining crate");
}

#[test]
fn generated_macro_definitions_keep_dollar_crate_from_original_macro() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /crates/dep/src/lib.rs crate:dep root:crates/dep
pub mod source {
    pub struct Thing;
}

#[macro_export]
macro_rules! define_inner {
    () => {
        macro_rules! inner {
            () => {
                pub use $crate::source::Thing;
            };
        }
    };
}

//- /crates/app/src/lib.rs crate:app root:crates/app deps:dep
use dep::define_inner;

define_inner!();
inner!();
"#,
    );
    let target = project.lib("app");

    target.entry("Thing").assert_type_exists(
        "generated macro definitions should preserve the original macro's $crate target",
    );
}

#[test]
fn expands_imported_macro_rules_items() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:imported_macro_fixture
mod macros {
    macro_rules! make_user {
        () => {
            pub struct User;
        };
    }

    pub(crate) use make_user;
}

use macros::make_user;

make_user!();
"#,
    );
    let target = project.lib("imported_macro_fixture");

    target
        .entry("User")
        .assert_type_exists("imported macro_rules bindings should expand after import resolution");
}

#[test]
fn expands_macros_generated_by_macros() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:nested_macro_fixture
macro_rules! define_make_user {
    () => {
        macro_rules! make_user {
            () => {
                pub struct User;
            };
        }
    };
}

define_make_user!();
make_user!();
"#,
    );
    let target = project.lib("nested_macro_fixture");

    target.entry("User").assert_type_exists(
        "a generated macro definition should be available to later item-position calls",
    );
}

#[test]
fn stops_recursive_generated_macro_expansion_at_pass_limit() {
    let run = rg_profile::test_support::ProfileTest::start(
        profile_descriptors(),
        "def_map.construction,def_map.macros",
    );
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:recursive_macro_fixture
macro_rules! recurse {
    () => {
        recurse!();
    };
}

macro_rules! finite_start {
    () => {
        finite_end!();
    };
}

macro_rules! finite_end {
    () => {
        pub struct FiniteResult;
    };
}

recurse!();
finite_start!();

pub struct After;
"#,
    );
    let snapshot = run.finish();
    let target = project.lib("recursive_macro_fixture");

    target
        .entry("After")
        .assert_type_exists("macro expansion limit should not abort DefMap construction");
    target
        .entry("FiniteResult")
        .assert_type_exists("an independent finite chain should complete before the limit");
    snapshot.assert_gauge_bool_with_message(
        metric::EXPANSION_PASS_LIMIT_REACHED,
        true,
        "recursive macro expansion should mark the pass limit as reached",
    );
    snapshot.assert_counter_with_message(
        metric::EXPANSION_PASSES,
        128,
        "recursive macro expansion should stop at the configured pass limit",
    );
    snapshot.assert_counter_satisfies_with_message(
        metric::MACRO_CALLS_SKIPPED_BY_LIMIT,
        |skipped| skipped > 0,
        "recursive macro expansion should leave some retryable calls skipped by limit",
    );

    let reports = project
        .def_map_db()
        .macro_expansion_limit_reports()
        .collect::<Vec<_>>();
    let [report] = reports.as_slice() else {
        panic!("the affected crate should retain one bounded macro-limit report");
    };
    assert_eq!(report.package_name, "recursive_macro_fixture");
    assert_eq!(report.crate_name, "recursive_macro_fixture");
    let [recursive] = report.groups.as_slice() else {
        panic!("only recursive fallout should remain pending at the limit");
    };
    assert_eq!(recursive.macro_name, "recurse");
    assert_eq!(recursive.source_call_count, 0);
    assert_eq!(recursive.generated_call_count, 1);
    assert!(recursive.chain_truncated);
    assert!(recursive.example_chain.iter().all(|name| name == "recurse"));
}

#[test]
fn qualified_macro_can_use_builtin_macro_name() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:qualified_builtin_name_fixture
mod macros {
    macro_rules! include {
        () => {
            pub struct QualifiedInclude;
        };
    }

    pub(crate) use include;
}

macros::include!();
"#,
    );
    let target = project.lib("qualified_builtin_name_fixture");

    target.entry("QualifiedInclude").assert_type_exists(
        "qualified user macros should not be classified as builtins by last segment",
    );
}

#[test]
fn unqualified_macro_can_resolve_from_standard_prelude() {
    let project = utils::DefMapFixtureDb::build_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:prelude_macro_fixture
cfg_select! {
    _ => { pub struct FromPrelude; },
}
"#,
    );
    let target = project.lib("prelude_macro_fixture");

    target
        .entry("FromPrelude")
        .assert_type_exists("unqualified macro calls should see standard-prelude macro bindings");
}

#[test]
fn imported_macro_shadows_standard_prelude_macro() {
    let project = utils::DefMapFixtureDb::build_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:prelude_macro_shadow_fixture
mod local {
    macro_rules! cfg_select {
        ($($tt:tt)*) => {
            pub struct FromLocal;
        };
    }

    pub(crate) use cfg_select;
}

use local::cfg_select;

cfg_select! {
    _ => { pub struct FromPrelude; },
}
"#,
    );
    let target = project.lib("prelude_macro_shadow_fixture");

    target
        .entry("FromLocal")
        .assert_type_exists("imported macros should shadow standard-prelude macros");
    target
        .entry("FromPrelude")
        .assert_missing("shadowed prelude macros should not expand");
}

#[test]
fn standard_prelude_macro_resolves_before_later_local_macro() {
    let project = utils::DefMapFixtureDb::build_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:prelude_before_later_local_fixture
cfg_select! {
    _ => { pub struct FromPrelude; },
}

macro_rules! cfg_select {
    ($($tt:tt)*) => {
        pub struct FromLaterLocal;
    };
}
"#,
    );
    let target = project.lib("prelude_before_later_local_fixture");

    target.entry("FromPrelude").assert_type_exists(
        "later same-module macro_rules bindings should be filtered before trying the prelude",
    );
    target.entry("FromLaterLocal").assert_missing(
        "later same-module macro_rules bindings should not expand the earlier call",
    );
}

#[test]
fn standard_prelude_macro_shadows_macro_use_extern_crate_fallback() {
    let project = utils::DefMapFixtureDb::build_with_fake_sysroot(
        r#"
//- /crates/dep/src/lib.rs crate:dep root:crates/dep
#[macro_export]
macro_rules! cfg_select {
    ($($tt:tt)*) => {
        pub struct FromMacroUse;
    };
}

//- /crates/app/src/lib.rs crate:app root:crates/app deps:dep
#[macro_use]
extern crate dep as _;

cfg_select! {
    _ => { pub struct FromPrelude; },
}
"#,
    );
    let target = project.lib("app");

    target
        .entry("FromPrelude")
        .assert_type_exists("standard-prelude macros should resolve before legacy macro_use");
    target
        .entry("FromMacroUse")
        .assert_missing("macro_use fallback should not run after a prelude macro resolves");
}

#[test]
fn standard_prelude_macro_resolves_before_macro_use_when_local_macro_is_later() {
    let project = utils::DefMapFixtureDb::build_with_fake_sysroot(
        r#"
//- /crates/dep/src/lib.rs crate:dep root:crates/dep
#[macro_export]
macro_rules! cfg_select {
    ($($tt:tt)*) => {
        pub struct FromMacroUse;
    };
}

//- /crates/app/src/lib.rs crate:app root:crates/app deps:dep
#[macro_use]
extern crate dep as _;

cfg_select! {
    _ => { pub struct FromPrelude; },
}

macro_rules! cfg_select {
    ($($tt:tt)*) => {
        pub struct FromLaterLocal;
    };
}
"#,
    );
    let target = project.lib("app");

    target.entry("FromPrelude").assert_type_exists(
        "prelude macros should resolve after filtering later locals and before macro_use fallback",
    );
    target.entry("FromLaterLocal").assert_missing(
        "later same-module macro_rules bindings should not expand the earlier call",
    );
    target
        .entry("FromMacroUse")
        .assert_missing("macro_use fallback should not win after a prelude macro resolves");
}

#[test]
fn conflicting_standard_prelude_macros_keep_the_first_before_macro_use_fallback() {
    let project = utils::DefMapFixtureDb::build_with_sysroot(
        r#"
//- /crates/dep/src/lib.rs crate:dep root:crates/dep
#[macro_export]
macro_rules! make_item {
    () => {
        pub struct FromMacroUse;
    };
}

//- /crates/app/src/lib.rs crate:app root:crates/app deps:dep
#[macro_use]
extern crate dep as _;

make_item!();

//- /sysroot/library/core/src/lib.rs
#[macro_export]
macro_rules! make_item {
    () => {
        pub struct FromFirstPrelude;
    };
}

//- /sysroot/library/alloc/src/lib.rs
pub struct Alloc;

//- /sysroot/library/std/src/lib.rs
#[macro_export]
macro_rules! make_item {
    () => {
        pub struct FromSecondPrelude;
    };
}

pub mod prelude {
    pub mod rust_2024 {
        pub use core::make_item;
        pub use crate::make_item;
    }
}

//- /sysroot/library/proc_macro/src/lib.rs
pub struct TokenStream;
"#,
    );
    let target = project.lib("app");

    target
        .entry("FromFirstPrelude")
        .assert_type_exists("conflicting prelude macros should expand the first useful target");
    target
        .entry("FromSecondPrelude")
        .assert_missing("the first prelude macro should win the conflict");
    target
        .entry("FromMacroUse")
        .assert_missing("the selected prelude macro should take precedence over legacy fallback");
}

#[test]
fn generated_extern_crate_preserves_aliases_and_macro_use() {
    utils::check_project_path_resolution(
        r#"
//- /dep/src/lib.rs crate:dep root:dep
pub struct Dependency;
#[macro_export]
macro_rules! declare { () => { pub struct Generated; }; }

//- /app/src/lib.rs crate:app root:app deps:dep
macro_rules! import_dep {
    () => { #[macro_use] extern crate dep as api; };
}
import_dep!();

mod child { pub use api::Dependency; }
pub use child::Dependency;
declare!();
"#,
        &[
            utils::PathResolutionQuery::lib("app", "crate", "api::Dependency").types(),
            utils::PathResolutionQuery::lib("app", "crate::child", "api::Dependency").types(),
            utils::PathResolutionQuery::lib("app", "crate", "Dependency").types(),
            utils::PathResolutionQuery::lib("app", "crate", "Generated").types(),
        ],
        expect![[r#"
            app [lib] crate resolves api::Dependency [types] -> struct dep[lib]::crate::Dependency
            app [lib] crate::child resolves api::Dependency [types] -> struct dep[lib]::crate::Dependency
            app [lib] crate resolves Dependency [types] -> struct dep[lib]::crate::Dependency
            app [lib] crate resolves Generated [types] -> struct app[lib]::crate::Generated
        "#]],
    );
}

#[test]
fn glob_updates_follow_the_winning_route_independently_of_attributes() {
    utils::check_project_path_resolution(
        r#"
//- /src/lib.rs crate:glob_route_refresh
mod original { pub struct Item; }
mod bridge {
    pub use crate::original::*;
    macro_rules! declare_item { () => { pub struct Item; }; }
    declare_item!();
}
mod facade {
    #[doc(hidden)]
    pub use crate::bridge::*;
    pub use crate::original::*;
}
"#,
        &[utils::PathResolutionQuery::lib("glob_route_refresh", "crate", "facade::Item").types()],
        expect![[r#"
            glob_route_refresh [lib] crate resolves facade::Item [types] -> struct glob_route_refresh[lib]::crate::bridge::Item
        "#]],
    );
}
