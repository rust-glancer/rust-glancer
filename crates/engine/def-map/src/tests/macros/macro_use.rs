use super::super::utils;

#[test]
fn macro_use_module_imports_child_macro_rules_into_parent_textual_scope() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "macro_use_module_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
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
fn local_macro_import_shadows_macro_use_extern_crate_fallback() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep1", "crates/dep2", "crates/app"]
resolver = "3"

//- /crates/dep1/Cargo.toml
[package]
name = "dep1"
version = "0.1.0"
edition = "2024"

//- /crates/dep1/src/lib.rs
#[macro_export]
macro_rules! m {
    () => {
        pub struct FromDep1;
    };
}

//- /crates/dep2/Cargo.toml
[package]
name = "dep2"
version = "0.1.0"
edition = "2024"

//- /crates/dep2/src/lib.rs
#[macro_export]
macro_rules! m {
    () => {
        pub struct FromDep2;
    };
}

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep1 = { path = "../dep1" }
dep2 = { path = "../dep2" }

//- /crates/app/src/lib.rs
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
#[macro_export]
macro_rules! make_dep {
    ($name:ident) => {
        pub struct $name;
    };
}

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
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
