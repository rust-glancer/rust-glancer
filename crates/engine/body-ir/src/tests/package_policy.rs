use crate::BodyIrBuildPolicy;
use expect_test::expect;

use super::utils::{check_project_body_ir, check_project_body_ir_with_policy};

#[test]
fn skips_non_workspace_package_bodies_by_default() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_policy_app"
version = "0.1.0"
edition = "2024"

[dependencies]
body_policy_dep = { path = "dep" }

//- /src/lib.rs
pub fn app() {}

//- /dep/Cargo.toml
[package]
name = "body_policy_dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub fn dep() {}
"#,
        expect![[r#"
            package body_policy_app

            body_policy_app [lib]
            body b0 fn body_policy_app[lib]::crate::app @ 1:1-1:16
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e0 block s1 => () @ 1:14-1:16


            package body_policy_dep

            body_policy_dep [lib]
            skipped
        "#]],
    );
}

#[test]
fn can_lower_non_workspace_package_bodies_when_requested() {
    check_project_body_ir_with_policy(
        r#"
//- /Cargo.toml
[package]
name = "body_policy_app"
version = "0.1.0"
edition = "2024"

[dependencies]
body_policy_dep = { path = "dep" }

//- /src/lib.rs
pub fn app() {}

//- /dep/Cargo.toml
[package]
name = "body_policy_dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub fn dep() {}
"#,
        BodyIrBuildPolicy::all_packages(),
        expect![[r#"
            package body_policy_app

            body_policy_app [lib]
            body b0 fn body_policy_app[lib]::crate::app @ 1:1-1:16
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e0 block s1 => () @ 1:14-1:16


            package body_policy_dep

            body_policy_dep [lib]
            body b0 fn body_policy_dep[lib]::crate::dep @ 1:1-1:16
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: <none>
            bindings
            body
            expr e0 block s1 => () @ 1:14-1:16
        "#]],
    );
}
