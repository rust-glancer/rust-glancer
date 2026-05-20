use super::utils;

#[test]
fn expands_local_macro_rules_items() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
}

#[test]
fn parent_textual_macro_rules_is_visible_in_later_child_module() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "textual_parent_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_user {
    () => {
        pub struct User;
    };
}

mod child {
    make_user!();
}

pub use child::User;
"#,
    );
    let target = project.lib("textual_parent_macro_fixture");

    target
        .entry("User")
        .assert_type_exists("parent textual macro_rules should be visible in later child modules");
}

#[test]
fn parent_textual_macro_rules_is_not_visible_in_earlier_child_module() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "textual_late_parent_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod child {
    make_hidden!();
}

macro_rules! make_hidden {
    () => {
        pub struct Hidden;
    };
}

pub use child::Hidden;
"#,
    );
    let target = project.lib("textual_late_parent_macro_fixture");

    target
        .entry("Hidden")
        .assert_missing("parent textual macro_rules should not be visible before its definition");
}

#[test]
fn same_module_textual_macro_rules_uses_latest_prior_definition() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "textual_macro_shadow_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make {
    () => {
        pub struct A;
    };
}

make!();

macro_rules! make {
    () => {
        pub struct B;
    };
}

make!();
"#,
    );
    let target = project.lib("textual_macro_shadow_fixture");

    target
        .entry("A")
        .assert_type_exists("the first call should use the first textual definition");
    target
        .entry("B")
        .assert_type_exists("the second call should use the later textual definition");
}

#[test]
fn inner_textual_macro_rules_shadows_parent_textual_macro() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "inner_textual_macro_shadow_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make {
    () => {
        pub struct Parent;
    };
}

mod child {
    macro_rules! make {
        () => {
            pub struct Child;
        };
    }

    make!();
}

pub use child::Child;
pub use child::Parent;
"#,
    );
    let target = project.lib("inner_textual_macro_shadow_fixture");

    target
        .entry("Child")
        .assert_type_exists("child textual macro_rules should shadow the parent definition");
    target
        .entry("Parent")
        .assert_missing("the parent textual macro should not be used when child has a match");
}

#[test]
fn resolves_imports_generated_by_macros() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "macro_import_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
fn resolves_dollar_crate_in_generated_imports_to_macro_definition_crate() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub mod source {
    pub struct Thing;
}

macro_rules! import_thing {
    () => {
        pub use $crate::source::Thing;
    };
}

pub use import_thing;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
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
fn expands_imported_macro_rules_items() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "imported_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
//- /Cargo.toml
[package]
name = "nested_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
    let (project, stats) = utils::DefMapFixtureDb::build_with_finalization_stats(
        r#"
//- /Cargo.toml
[package]
name = "recursive_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! recurse {
    () => {
        recurse!();
    };
}

recurse!();

pub struct After;
"#,
    );
    let target = project.lib("recursive_macro_fixture");

    target
        .entry("After")
        .assert_type_exists("macro expansion limit should not abort def-map finalization");
    assert!(stats.expansion_pass_limit_reached);
    assert_eq!(stats.expansion_passes, stats.expansion_pass_limit);
    assert!(stats.macro_calls_skipped_by_limit > 0);
}

#[test]
fn local_macro_can_shadow_builtin_macro_name() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "builtin_shadow_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! include {
    () => {
        pub struct LocalInclude;
    };
}

include!();
"#,
    );
    let target = project.lib("builtin_shadow_fixture");

    target.entry("LocalInclude").assert_type_exists(
        "resolved local macros should expand even when their name matches a builtin macro",
    );
}

#[test]
fn qualified_macro_can_use_builtin_macro_name() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "qualified_builtin_name_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
        "qualified user macros should not be treated as unsupported builtins by last segment",
    );
}

#[test]
fn skips_cfg_disabled_macro_definitions_and_calls() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "cfg_macro_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
#[cfg(false)]
macro_rules! make_hidden {
    () => {
        pub struct Hidden;
    };
}

#[cfg(false)]
make_hidden!();

#[cfg(true)]
macro_rules! make_visible {
    () => {
        pub struct Visible;
    };
}

make_visible!();
"#,
    );
    let target = project.lib("cfg_macro_fixture");

    target
        .entry("Hidden")
        .assert_missing("cfg-disabled macro calls should not contribute generated definitions");
    target
        .entry("make_hidden")
        .assert_missing("cfg-disabled macro definitions should not bind");
    target
        .entry("Visible")
        .assert_type_exists("cfg-enabled macro calls should still expand");
}

#[test]
fn filters_macro_generated_cfg_items() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_cfg_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_items {
    () => {
        #[cfg(false)]
        pub struct Hidden;

        #[cfg(true)]
        pub struct Visible;
    };
}

make_items!();
"#,
    );
    let target = project.lib("generated_cfg_fixture");

    target
        .entry("Hidden")
        .assert_missing("cfg-disabled generated items should not be collected");
    target
        .entry("Visible")
        .assert_type_exists("cfg-enabled generated items should be collected");
}

#[test]
fn filters_feature_cfg_items_with_cargo_active_features() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "feature_cfg_fixture"
version = "0.1.0"
edition = "2024"

[features]
default = ["enabled"]
enabled = []
disabled = []

//- /src/lib.rs
#[cfg(feature = "enabled")]
pub struct Enabled;

#[cfg(feature = "disabled")]
pub struct Disabled;
"#,
    );
    let target = project.lib("feature_cfg_fixture");

    target
        .entry("Enabled")
        .assert_type_exists("default Cargo features should be present in cfg options");
    target
        .entry("Disabled")
        .assert_missing("inactive Cargo features should not satisfy cfg predicates");
}

#[test]
fn filters_current_platform_cfg_items() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "platform_cfg_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
#[cfg(unix)]
pub struct UnixOnly;

#[cfg(windows)]
pub struct WindowsOnly;
"#,
    );
    let target = project.lib("platform_cfg_fixture");

    if cfg!(unix) {
        target
            .entry("UnixOnly")
            .assert_type_exists("host unix cfg should be present");
        target
            .entry("WindowsOnly")
            .assert_missing("host windows cfg should be absent on unix");
    } else if cfg!(windows) {
        target
            .entry("UnixOnly")
            .assert_missing("host unix cfg should be absent on windows");
        target
            .entry("WindowsOnly")
            .assert_type_exists("host windows cfg should be present");
    }
}

#[test]
fn applies_cfg_attr_cfg_gates() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "cfg_attr_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
#[cfg_attr(true, cfg(false))]
pub struct Hidden;

#[cfg_attr(false, cfg(false))]
pub struct Visible;
"#,
    );
    let target = project.lib("cfg_attr_fixture");

    target
        .entry("Hidden")
        .assert_missing("active cfg_attr cfg gates should be applied");
    target
        .entry("Visible")
        .assert_type_exists("inactive cfg_attr cfg gates should be ignored");
}
