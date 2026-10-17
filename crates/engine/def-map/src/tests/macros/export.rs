use crate::tests::utils;

#[test]
fn macro_export_makes_macro_rules_visible_from_crate_root() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /crates/dep/src/lib.rs crate:dep root:crates/dep
pub mod source {
    pub struct Thing;
}

mod private_macros {
    #[macro_export]
    macro_rules! export_thing {
        ($name:ident) => {
            pub use $crate::source::Thing as $name;
        };
    }
}

//- /crates/app/src/lib.rs crate:app root:crates/app deps:dep
use dep::export_thing;

dep::export_thing!(DirectThing);
export_thing!(ImportedThing);
"#,
    );
    let target = project.lib("app");

    target
        .entry("DirectThing")
        .assert_type_exists("qualified #[macro_export] macros should resolve from crate root");
    target
        .entry("ImportedThing")
        .assert_type_exists("imported #[macro_export] macros should behave like root exports");
}

#[test]
fn macro_exported_root_macro_can_be_called_through_crate_path() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:root_macro_export_fixture
#[macro_export]
macro_rules! exported {
    () => {
        pub struct RootExported;
    };
}

crate::exported!();
"#,
    );
    let target = project.lib("root_macro_export_fixture");

    target.entry("RootExported").assert_type_exists(
        "root #[macro_export] macros should not become ambiguous with their normal binding",
    );
}

#[test]
fn macro_exported_macro_can_be_called_through_crate_path_before_definition() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:early_root_macro_export_fixture
crate::exported!();

#[macro_export]
macro_rules! exported {
    () => {
        pub struct EarlyExported;
    };
}
"#,
    );
    let target = project.lib("early_root_macro_export_fixture");

    target.entry("EarlyExported").assert_type_exists(
        "#[macro_export] crate-root path bindings should not be filtered by definition order",
    );
}

#[test]
fn cfg_attr_macro_export_respects_active_and_inactive_predicates() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:cfg_attr_macro_export_fixture
crate::active!();
crate::inactive!();

#[cfg_attr(true, macro_export)]
macro_rules! active {
    () => {
        pub struct ActiveExport;
    };
}

#[cfg_attr(false, macro_export)]
macro_rules! inactive {
    () => {
        pub struct InactiveExport;
    };
}
"#,
    );
    let target = project.lib("cfg_attr_macro_export_fixture");

    target
        .entry("ActiveExport")
        .assert_type_exists("active cfg_attr macro_export should expose a root macro binding");
    target
        .entry("InactiveExport")
        .assert_missing("inactive cfg_attr macro_export should not expose a root macro binding");
}

#[test]
fn non_exported_macro_path_call_before_definition_is_not_visible() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:early_private_macro_path_fixture
crate::make_hidden!();

macro_rules! make_hidden {
    () => {
        pub struct Hidden;
    };
}
"#,
    );
    let target = project.lib("early_private_macro_path_fixture");

    target
        .entry("Hidden")
        .assert_missing("direct macro_rules path bindings should remain source-order sensitive");
}

#[test]
fn generated_macro_export_makes_macro_visible_from_crate_root() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:generated_macro_export_fixture
macro_rules! define_exported {
    () => {
        #[cfg_attr(true, macro_export)]
        macro_rules! exported {
            () => {
                pub struct GeneratedExport;
            };
        }
    };
}

mod child {
    define_exported!();
}

exported!();
"#,
    );
    let target = project.lib("generated_macro_export_fixture");

    target.entry("GeneratedExport").assert_type_exists(
        "generated cfg_attr macro_export definitions should be inserted into the crate root",
    );
}
