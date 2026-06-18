use super::super::utils;
use rg_workspace::WorkspaceLoweringConfig;

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
fn filters_test_cfg_items_with_workspace_config() {
    let fixture = r#"
//- /Cargo.toml
[package]
name = "test_cfg_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
#[cfg(test)]
pub struct TestOnly;

#[cfg(not(test))]
pub struct NormalOnly;
"#;

    let normal_project = utils::DefMapFixtureDb::build(fixture);
    let normal_target = normal_project.lib("test_cfg_fixture");
    normal_target
        .entry("TestOnly")
        .assert_missing("cfg(test) items should be hidden by default");
    normal_target
        .entry("NormalOnly")
        .assert_type_exists("cfg(not(test)) items should be present by default");

    let test_project = utils::DefMapFixtureDb::build_with_workspace_config(
        fixture,
        WorkspaceLoweringConfig::default().cfg_test(true),
    );
    let test_target = test_project.lib("test_cfg_fixture");
    test_target
        .entry("TestOnly")
        .assert_type_exists("cfg(test) items should be present when cfg(test) is enabled");
    test_target
        .entry("NormalOnly")
        .assert_missing("cfg(not(test)) items should be hidden when cfg(test) is enabled");
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
