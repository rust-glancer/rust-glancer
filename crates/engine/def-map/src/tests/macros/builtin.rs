use super::super::utils;
use expect_test::{Expect, expect};

const BUILTIN_MACRO_SYSROOT: &str = r#"
//- /sysroot/library/core/src/lib.rs
#[rustc_builtin_macro]
#[macro_export]
macro_rules! include {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! cfg_select {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

pub mod prelude {
    pub mod rust_2024 {
        pub use crate::cfg_select;
        pub use crate::include;
    }
}

//- /sysroot/library/alloc/src/lib.rs
pub struct Alloc;

//- /sysroot/library/std/src/lib.rs
#[rustc_builtin_macro]
#[macro_export]
macro_rules! include {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! cfg_select {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

pub mod prelude {
    pub mod rust_2024 {
        pub use crate::cfg_select;
        pub use crate::include;
    }
}
"#;

fn builtin_macro_fixture(fixture: &str) -> String {
    format!("{fixture}\n{BUILTIN_MACRO_SYSROOT}")
}

fn build_builtin_macro_fixture(fixture: &str) -> utils::DefMapFixtureDb {
    let fixture = builtin_macro_fixture(fixture);
    utils::DefMapFixtureDb::build_with_sysroot(&fixture)
}

fn check_builtin_macro_project_def_map(fixture: &str, expect: Expect) {
    let fixture = builtin_macro_fixture(fixture);
    utils::check_project_def_map_with_sysroot(&fixture, expect);
}

fn check_builtin_macro_path_resolution(
    fixture: &str,
    queries: &[utils::PathResolutionQuery],
    expect: Expect,
) {
    let fixture = builtin_macro_fixture(fixture);
    utils::check_project_path_resolution_with_sysroot(&fixture, queries, expect);
}

#[test]
fn include_macro_splices_real_source_items() {
    let project = build_builtin_macro_fixture(
        r#"
//- /Cargo.toml
[package]
name = "include_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
include!("included.rs");

make_included!();

//- /src/included.rs
pub struct Included;

macro_rules! make_included {
    () => {
        pub struct FromIncludedMacro;
    };
}
"#,
    );
    let target = project.lib("include_macro_fixture");

    target
        .entry("Included")
        .assert_type_exists("include should splice item definitions into the caller module")
        .assert_type_source_file("included.rs", "included items should keep real file spans");
    target.entry("FromIncludedMacro").assert_type_exists(
        "macro_rules definitions from included files should be visible to later calls",
    );
}

#[test]
fn qualified_include_macro_splices_real_source_items() {
    let project = build_builtin_macro_fixture(
        r#"
//- /Cargo.toml
[package]
name = "qualified_include_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
std::include!("included.rs");

//- /src/included.rs
pub struct Included;
"#,
    );
    let target = project.lib("qualified_include_macro_fixture");

    target
        .entry("Included")
        .assert_type_exists("std-qualified include should resolve the sysroot builtin");
}

#[test]
fn local_include_macro_shadows_builtin_include() {
    let project = build_builtin_macro_fixture(
        r#"
//- /Cargo.toml
[package]
name = "include_shadow_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! include {
    ($path:literal) => {
        pub struct FromMacro;
    };
}

include!("included.rs");

//- /src/included.rs
pub struct FromFile;
"#,
    );
    let target = project.lib("include_shadow_fixture");

    target
        .entry("FromMacro")
        .assert_type_exists("local macro_rules definitions should shadow builtin include");
    target
        .entry("FromFile")
        .assert_missing("shadowed include calls should not splice the referenced file");
}

#[test]
fn cfg_select_expands_first_enabled_item_arm() {
    let project = build_builtin_macro_fixture(
        r#"
//- /Cargo.toml
[package]
name = "cfg_select_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
cfg_select! {
    false => {
        pub struct HiddenFalse;
    }
    true => {
        pub struct Selected;

        macro_rules! make_from_selected {
            () => {
                pub struct FromSelectedMacro;
            };
        }
    }
    true => {
        pub struct HiddenLater;
    }
}

make_from_selected!();
"#,
    );
    let target = project.lib("cfg_select_fixture");

    target
        .entry("Selected")
        .assert_type_exists("cfg_select should collect the first enabled item arm");
    target
        .entry("FromSelectedMacro")
        .assert_type_exists("macro definitions from selected cfg_select arms should be usable");
    target
        .entry("HiddenFalse")
        .assert_missing("disabled cfg_select arms should not contribute items");
    target
        .entry("HiddenLater")
        .assert_missing("later enabled cfg_select arms should not be reached");
}

#[test]
fn qualified_cfg_select_expands_first_enabled_item_arm() {
    let project = build_builtin_macro_fixture(
        r#"
//- /Cargo.toml
[package]
name = "qualified_cfg_select_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
std::cfg_select! {
    true => {
        pub struct FromStd;
    }
}

core::cfg_select! {
    true => {
        pub struct FromCore;
    }
}

::std::cfg_select! {
    true => {
        pub struct FromAbsoluteStd;
    }
}
"#,
    );
    let target = project.lib("qualified_cfg_select_fixture");

    target
        .entry("FromStd")
        .assert_type_exists("std-qualified cfg_select should resolve the sysroot builtin");
    target
        .entry("FromCore")
        .assert_type_exists("core-qualified cfg_select should resolve the sysroot builtin");
    target
        .entry("FromAbsoluteStd")
        .assert_type_exists("absolute std-qualified cfg_select should resolve the sysroot builtin");
}

#[test]
fn cfg_select_uses_wildcard_fallback() {
    let project = build_builtin_macro_fixture(
        r#"
//- /Cargo.toml
[package]
name = "cfg_select_fallback_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
cfg_select! {
    false => {
        pub struct Hidden;
    }
    _ => {
        pub struct Fallback;
    }
}
"#,
    );
    let target = project.lib("cfg_select_fallback_fixture");

    target
        .entry("Fallback")
        .assert_type_exists("cfg_select wildcard arm should act as fallback");
    target
        .entry("Hidden")
        .assert_missing("inactive cfg_select arms should not contribute items");
}

#[test]
fn cfg_select_matches_key_value_cfg_predicates() {
    let project = build_builtin_macro_fixture(
        r#"
//- /Cargo.toml
[package]
name = "cfg_select_feature_fixture"
version = "0.1.0"
edition = "2024"

[features]
default = ["enabled"]
enabled = []

//- /src/lib.rs
cfg_select! {
    feature = "enabled" => {
        pub struct EnabledFeature;
    }
    _ => {
        pub struct Fallback;
    }
}
"#,
    );
    let target = project.lib("cfg_select_feature_fixture");

    target
        .entry("EnabledFeature")
        .assert_type_exists("cfg_select should compare key-value cfg payloads without quotes");
    target
        .entry("Fallback")
        .assert_missing("fallback should not be reached after a matching feature cfg");
}

#[test]
fn cfg_select_collects_out_of_line_modules_relative_to_call_site() {
    check_builtin_macro_path_resolution(
        r#"
//- /Cargo.toml
[package]
name = "cfg_select_module_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
cfg_select! {
    true => {
        mod os;
    }
}

//- /src/os.rs
pub struct Unix;
"#,
        &[utils::PathResolutionQuery::lib(
            "cfg_select_module_fixture",
            "crate::os",
            "Unix",
        )],
        expect![[r#"
            cfg_select_module_fixture [lib] crate::os resolves Unix -> struct cfg_select_module_fixture[lib]::crate::os::Unix
        "#]],
    );
}

#[test]
fn cfg_select_collects_impls_and_extern_crates_as_source_items() {
    check_builtin_macro_project_def_map(
        r#"
//- /Cargo.toml
[package]
name = "cfg_select_source_items_fixture"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "dep" }

//- /src/lib.rs
cfg_select! {
    true => {
        extern crate dep;

        pub struct User;

        impl User {}
    }
}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Dep;
"#,
        expect![[r#"
            package alloc

            alloc [lib]
            crate
            - Alloc : type [pub struct alloc[lib]::crate::Alloc]

            package cfg_select_source_items_fixture

            cfg_select_source_items_fixture [lib]
            crate
            - User : type [pub struct cfg_select_source_items_fixture[lib]::crate::User]
            - dep : type [module dep[lib]::crate]
            impls
            - impl lib.rs#2

            package core

            core [lib]
            crate
            - cfg_select : macro [macro_definition core[lib]::crate::cfg_select; pub macro_definition core[lib]::crate::cfg_select]
            - include : macro [macro_definition core[lib]::crate::include; pub macro_definition core[lib]::crate::include]
            - prelude : type [pub module core[lib]::crate::prelude]

            crate::prelude
            - rust_2024 : type [pub module core[lib]::crate::prelude::rust_2024]

            crate::prelude::rust_2024
            - cfg_select : macro [pub macro_definition core[lib]::crate::cfg_select]
            - include : macro [pub macro_definition core[lib]::crate::include]

            package dep

            dep [lib]
            crate
            - Dep : type [pub struct dep[lib]::crate::Dep]

            package std

            std [lib]
            crate
            - cfg_select : macro [macro_definition std[lib]::crate::cfg_select; pub macro_definition std[lib]::crate::cfg_select]
            - include : macro [macro_definition std[lib]::crate::include; pub macro_definition std[lib]::crate::include]
            - prelude : type [pub module std[lib]::crate::prelude]

            crate::prelude
            - rust_2024 : type [pub module std[lib]::crate::prelude::rust_2024]

            crate::prelude::rust_2024
            - cfg_select : macro [pub macro_definition std[lib]::crate::cfg_select]
            - include : macro [pub macro_definition std[lib]::crate::include]
        "#]],
    );
}

#[test]
fn cfg_select_ignores_failed_inactive_arm_lowering() {
    let project = build_builtin_macro_fixture(
        r#"
//- /Cargo.toml
[package]
name = "cfg_select_failed_inactive_arm_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
cfg_select! {
    false => {
        mod ;
    }
    true => {
        pub struct Selected;
    }
}
"#,
    );
    let target = project.lib("cfg_select_failed_inactive_arm_fixture");

    target.entry("Selected").assert_type_exists(
        "inactive cfg_select arm lowering failures should not poison selected arms",
    );
}

#[test]
fn local_cfg_select_macro_shadows_builtin_cfg_select() {
    let project = build_builtin_macro_fixture(
        r#"
//- /Cargo.toml
[package]
name = "cfg_select_shadow_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! cfg_select {
    ($($tt:tt)*) => {
        pub struct FromLocalMacro;
    };
}

cfg_select! {
    true => {
        pub struct FromBuiltin;
    }
}
"#,
    );
    let target = project.lib("cfg_select_shadow_fixture");

    target
        .entry("FromLocalMacro")
        .assert_type_exists("local macro_rules should shadow builtin cfg_select");
    target
        .entry("FromBuiltin")
        .assert_missing("shadowed builtin cfg_select should not run");
}
