use crate::tests::utils;

#[test]
fn macro_use_module_imports_child_macro_rules_into_parent_textual_scope() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:macro_use_module_fixture
#[macro_use]
mod macros {
    macro_rules! make_user {
        () => {
            pub struct User;
        };
    }

    macro_rules! make_hidden {
        () => {
            pub struct Hidden;
        };
    }
}

make_user!();
make_hidden!();
"#,
    );
    let target = project.lib("macro_use_module_fixture");

    target
        .entry("User")
        .assert_type_exists("macro_use modules should expose child macro_rules to the parent");
    target
        .entry("Hidden")
        .assert_type_exists("plain macro_use modules should expose all child macro_rules");
}

#[test]
fn macro_use_extern_crate_imports_exported_root_macros() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /crates/dep/src/lib.rs crate:dep root:crates/dep
pub mod source {
    pub struct Thing;
}

#[macro_export]
macro_rules! export_thing {
    ($name:ident) => {
        pub use $crate::source::Thing as $name;
    };
}

#[macro_export]
macro_rules! hidden_thing {
    ($name:ident) => {
        pub use $crate::source::Thing as $name;
    };
}

//- /crates/app/src/lib.rs crate:app root:crates/app deps:dep
#[macro_use(export_thing)]
extern crate dep as _;

export_thing!(ThingViaMacroUse);
hidden_thing!(HiddenViaMacroUse);

mod child {
    export_thing!(ChildThingViaMacroUse);
    hidden_thing!(ChildHiddenViaMacroUse);
}

pub use child::ChildThingViaMacroUse;
pub use child::ChildHiddenViaMacroUse;
"#,
    );
    let target = project.lib("app");

    target.entry("ThingViaMacroUse").assert_type_exists(
        "macro_use extern crate should import selected exported macros from the dependency root",
    );
    target.entry("HiddenViaMacroUse").assert_missing(
        "macro_use extern crate name lists should leave unlisted exported macros unavailable",
    );
    target.entry("ChildThingViaMacroUse").assert_type_exists(
        "macro_use extern crate should be visible from child modules as a crate-wide legacy prelude",
    );
    target
        .entry("ChildHiddenViaMacroUse")
        .assert_missing("macro_use extern crate name lists should also filter child-module calls");
}

#[test]
fn macro_use_extern_crate_conflicts_follow_declaration_order() {
    for (case, imports, expected, other) in [
        (
            "the later written import wins",
            "#[macro_use] extern crate first; #[macro_use] extern crate second;",
            "FromSecond",
            "FromFirst",
        ),
        (
            "reversing written declarations reverses the winner",
            "#[macro_use] extern crate second; #[macro_use] extern crate first;",
            "FromFirst",
            "FromSecond",
        ),
        (
            "an earlier generated import does not shadow a later written import",
            "import_first!(); #[macro_use] extern crate second;",
            "FromSecond",
            "FromFirst",
        ),
        (
            "a later generated import shadows an earlier written import",
            "#[macro_use] extern crate first; import_second!();",
            "FromSecond",
            "FromFirst",
        ),
    ] {
        let fixture = format!(
            r#"
//- /first/src/lib.rs crate:first root:first
#[macro_export]
macro_rules! shared {{ () => {{ pub struct FromFirst; }}; }}

//- /second/src/lib.rs crate:second root:second
#[macro_export]
macro_rules! shared {{ () => {{ pub struct FromSecond; }}; }}

//- /app/src/lib.rs crate:app root:app deps:first,second
macro_rules! import_first {{ () => {{ #[macro_use] extern crate first; }}; }}
macro_rules! import_second {{ () => {{ #[macro_use] extern crate second; }}; }}
{imports}

// Generate the consumer too, so it resolves after the generated extern crates are collected.
macro_rules! invoke_selected {{ () => {{ shared!(); }}; }}
invoke_selected!();
"#,
        );
        let project = utils::DefMapFixtureDb::build(&fixture);
        let target = project.lib("app");
        target.entry(expected).assert_type_exists(case);
        target.entry(other).assert_missing(case);
    }
}

#[test]
fn local_macro_import_shadows_macro_use_extern_crate_fallback() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /crates/dep1/src/lib.rs crate:dep1 root:crates/dep1
#[macro_export]
macro_rules! m {
    () => {
        pub struct FromDep1;
    };
}

//- /crates/dep2/src/lib.rs crate:dep2 root:crates/dep2
#[macro_export]
macro_rules! m {
    () => {
        pub struct FromDep2;
    };
}

//- /crates/app/src/lib.rs crate:app root:crates/app deps:dep1,dep2
#[macro_use]
extern crate dep1;
extern crate dep2;

mod child {
    use dep2::m;

    m!();
}

pub use child::FromDep1;
pub use child::FromDep2;
"#,
    );
    let target = project.lib("app");

    target.entry("FromDep2").assert_type_exists(
        "local macro imports should shadow macro_use extern crate fallback bindings",
    );
    target.entry("FromDep1").assert_missing(
        "macro_use extern crate fallback should not compete with local macro imports",
    );
}

#[test]
fn cfg_attr_macro_use_activates_legacy_macro_imports() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /crates/dep/src/lib.rs crate:dep root:crates/dep
#[macro_export]
macro_rules! make_dep {
    ($name:ident) => {
        pub struct $name;
    };
}

//- /crates/app/src/lib.rs crate:app root:crates/app deps:dep
#[cfg_attr(true, macro_use(make_dep))]
extern crate dep as _;

#[cfg_attr(true, macro_use)]
mod local_macros {
    macro_rules! make_local {
        () => {
            pub struct Local;
        };
    }
}

#[cfg_attr(false, macro_use)]
mod inactive_macros {
    macro_rules! make_hidden {
        () => {
            pub struct Hidden;
        };
    }
}

make_dep!(FromDep);
make_local!();
make_hidden!();
"#,
    );
    let target = project.lib("app");

    target
        .entry("FromDep")
        .assert_type_exists("active cfg_attr macro_use should import extern crate macros");
    target
        .entry("Local")
        .assert_type_exists("active cfg_attr macro_use should import child module macro_rules");
    target
        .entry("Hidden")
        .assert_missing("inactive cfg_attr macro_use should not import child module macro_rules");
}
