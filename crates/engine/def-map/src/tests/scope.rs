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
