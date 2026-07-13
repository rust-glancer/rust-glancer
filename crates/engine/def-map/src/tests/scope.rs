use expect_test::expect;

use super::utils;

#[test]
fn private_items_are_not_visible_to_sibling_modules() {
    utils::check_project_def_map(
        r#"
//- /Cargo.toml
[package]
name = "sibling_private_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod source {
    fn hidden() {}
    pub fn exposed() {}
}

mod sibling {
    use crate::source::{exposed, hidden};
}
"#,
        expect![[r#"
            package sibling_private_fixture

            sibling_private_fixture [lib]
            crate
            - sibling : type [module sibling_private_fixture[lib]::crate::sibling]
            - source : type [module sibling_private_fixture[lib]::crate::source]

            crate::sibling
            - exposed : value [fn sibling_private_fixture[lib]::crate::source::exposed]
            unresolved imports
            - use crate::source::hidden

            crate::source
            - exposed : value [pub fn sibling_private_fixture[lib]::crate::source::exposed]
            - hidden : value [fn sibling_private_fixture[lib]::crate::source::hidden]
        "#]],
    );
}

#[test]
fn child_modules_can_see_private_items_from_ancestor_modules() {
    utils::check_project_def_map(
        r#"
//- /Cargo.toml
[package]
name = "ancestor_private_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod parent {
    fn hidden() {}

    mod child {
        use super::hidden;
    }
}
"#,
        expect![[r#"
            package ancestor_private_fixture

            ancestor_private_fixture [lib]
            crate
            - parent : type [module ancestor_private_fixture[lib]::crate::parent]

            crate::parent
            - child : type [module ancestor_private_fixture[lib]::crate::parent::child]
            - hidden : value [fn ancestor_private_fixture[lib]::crate::parent::hidden]

            crate::parent::child
            - hidden : value [fn ancestor_private_fixture[lib]::crate::parent::hidden]
        "#]],
    );
}

#[test]
fn restricted_visibility_is_evaluated_from_the_binding_owner_module() {
    utils::check_project_def_map(
        r#"
//- /Cargo.toml
[package]
name = "restricted_visibility_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod api {
    pub(super) fn visible_to_root_children() {}
    pub(self) fn private_to_api_descendants() {}
    pub(in crate::api) fn visible_to_api_descendants() {}
    pub fn visible_in_crate() {}

    mod child {
        use super::visible_to_api_descendants;
    }
}

mod sibling {
    use crate::api::{
        private_to_api_descendants,
        visible_to_api_descendants,
        visible_in_crate,
        visible_to_root_children,
    };
}
"#,
        expect![[r#"
            package restricted_visibility_fixture

            restricted_visibility_fixture [lib]
            crate
            - api : type [module restricted_visibility_fixture[lib]::crate::api]
            - sibling : type [module restricted_visibility_fixture[lib]::crate::sibling]

            crate::api
            - child : type [module restricted_visibility_fixture[lib]::crate::api::child]
            - private_to_api_descendants : value [pub(self) fn restricted_visibility_fixture[lib]::crate::api::private_to_api_descendants]
            - visible_in_crate : value [pub fn restricted_visibility_fixture[lib]::crate::api::visible_in_crate]
            - visible_to_api_descendants : value [pub(in crate::api) fn restricted_visibility_fixture[lib]::crate::api::visible_to_api_descendants]
            - visible_to_root_children : value [pub(super) fn restricted_visibility_fixture[lib]::crate::api::visible_to_root_children]

            crate::api::child
            - visible_to_api_descendants : value [fn restricted_visibility_fixture[lib]::crate::api::visible_to_api_descendants]

            crate::sibling
            - visible_in_crate : value [fn restricted_visibility_fixture[lib]::crate::api::visible_in_crate]
            - visible_to_root_children : value [fn restricted_visibility_fixture[lib]::crate::api::visible_to_root_children]
            unresolved imports
            - use crate::api::private_to_api_descendants
            - use crate::api::visible_to_api_descendants
        "#]],
    );
}

#[test]
fn restricted_visibility_canonicalizes_raw_identifier_segments() {
    utils::check_project_def_map(
        r#"
//- /Cargo.toml
[package]
name = "raw_restricted_visibility_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod r#type {
    pub(in crate::r#type) fn visible_to_type_descendants() {}

    mod child {
        use super::visible_to_type_descendants;
    }
}
"#,
        expect![[r#"
            package raw_restricted_visibility_fixture

            raw_restricted_visibility_fixture [lib]
            crate
            - type : type [module raw_restricted_visibility_fixture[lib]::crate::type]

            crate::type
            - child : type [module raw_restricted_visibility_fixture[lib]::crate::type::child]
            - visible_to_type_descendants : value [pub(in crate::r#type) fn raw_restricted_visibility_fixture[lib]::crate::type::visible_to_type_descendants]

            crate::type::child
            - visible_to_type_descendants : value [fn raw_restricted_visibility_fixture[lib]::crate::type::visible_to_type_descendants]
        "#]],
    );
}

#[test]
fn keyword_module_imports_keep_the_module_declaration_visibility_ceiling() {
    utils::check_project_def_map(
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
mod parent {
    pub mod child {
        pub use super as parent_alias;
    }
}

pub use parent::child;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
use dep::child::parent_alias;
"#,
        expect![[r#"
            package app

            app [lib]
            crate
            unresolved imports
            - use dep::child::parent_alias

            package dep

            dep [lib]
            crate
            - child : type [pub module dep[lib]::crate::parent::child]
            - parent : type [module dep[lib]::crate::parent]

            crate::parent
            - child : type [pub module dep[lib]::crate::parent::child]

            crate::parent::child
            - parent_alias : type [module dep[lib]::crate::parent]
        "#]],
    );
}

#[test]
fn only_macro_rules_direct_bindings_receive_the_crate_reexport_ceiling() {
    utils::check_project_def_map(
        r#"
//- /Cargo.toml
[package]
name = "macro_reexport_visibility_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod source {
    macro_rules! legacy {
        () => {};
    }

    macro macro2 {
        () => {};
    }

    pub use legacy as legacy_export;
    pub(crate) use macro2 as macro2_export;
}

mod sibling {
    use crate::source::{legacy_export, macro2_export};
}
"#,
        expect![[r#"
            package macro_reexport_visibility_fixture

            macro_reexport_visibility_fixture [lib]
            crate
            - sibling : type [module macro_reexport_visibility_fixture[lib]::crate::sibling]
            - source : type [module macro_reexport_visibility_fixture[lib]::crate::source]

            crate::sibling
            - legacy_export : macro [macro_definition macro_reexport_visibility_fixture[lib]::crate::source::legacy]
            unresolved imports
            - use crate::source::macro2_export

            crate::source
            - legacy : macro [macro_definition macro_reexport_visibility_fixture[lib]::crate::source::legacy]
            - legacy_export : macro [macro_definition macro_reexport_visibility_fixture[lib]::crate::source::legacy]
            - macro2 : macro [macro_definition macro_reexport_visibility_fixture[lib]::crate::source::macro2]
            - macro2_export : macro [macro_definition macro_reexport_visibility_fixture[lib]::crate::source::macro2]
        "#]],
    );
}

#[test]
fn public_reexports_do_not_make_inaccessible_source_bindings_visible() {
    utils::check_project_def_map(
        r#"
//- /Cargo.toml
[package]
name = "reexport_visibility_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod source {
    fn hidden() {}
    pub fn exposed() {}
}

mod reexports {
    pub use crate::source::{exposed, hidden};
}

use crate::reexports::{exposed, hidden};
"#,
        expect![[r#"
            package reexport_visibility_fixture

            reexport_visibility_fixture [lib]
            crate
            - exposed : value [fn reexport_visibility_fixture[lib]::crate::source::exposed]
            - reexports : type [module reexport_visibility_fixture[lib]::crate::reexports]
            - source : type [module reexport_visibility_fixture[lib]::crate::source]
            unresolved imports
            - use crate::reexports::hidden

            crate::reexports
            - exposed : value [pub fn reexport_visibility_fixture[lib]::crate::source::exposed]
            unresolved imports
            - pub use crate::source::hidden

            crate::source
            - exposed : value [pub fn reexport_visibility_fixture[lib]::crate::source::exposed]
            - hidden : value [fn reexport_visibility_fixture[lib]::crate::source::hidden]
        "#]],
    );
}

#[test]
fn public_reexports_do_not_widen_crate_visibility_across_crates() {
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
mod source {
    pub(crate) struct Thing;
}

pub use source::Thing;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
use dep::Thing;
"#,
    );

    project.lib("dep").entry("Thing").assert_type_exists(
        "the re-export remains usable inside the crate where its source is visible",
    );
    project
        .lib("app")
        .entry("Thing")
        .assert_missing("a public import must not widen a crate-visible source across crates");
}
