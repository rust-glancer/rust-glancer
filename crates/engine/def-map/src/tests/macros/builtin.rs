use super::super::utils;

#[test]
fn include_macro_splices_real_source_items() {
    let project = utils::DefMapFixtureDb::build(
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
fn local_include_macro_shadows_builtin_include() {
    let project = utils::DefMapFixtureDb::build(
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
    let project = utils::DefMapFixtureDb::build(
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
fn cfg_select_uses_wildcard_fallback() {
    let project = utils::DefMapFixtureDb::build(
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
    let project = utils::DefMapFixtureDb::build(
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
fn local_cfg_select_macro_shadows_builtin_cfg_select() {
    let project = utils::DefMapFixtureDb::build(
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
