use expect_test::expect;

use super::utils::{check_project_body_ir, check_project_body_ir_with_policy};
use crate::BodyIrBuildPolicy;

const PACKAGE_POLICY_FIXTURE: &str = r#"
//- /src/lib.rs crate:body_policy_app deps:body_policy_dep
pub fn app() {}

//- /dep/src/lib.rs crate:body_policy_dep root:dep origin:dependency
pub fn dep() {}
"#;

#[test]
fn compares_default_and_all_packages_body_policies() {
    check_project_body_ir(
        PACKAGE_POLICY_FIXTURE,
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
    check_project_body_ir_with_policy(
        PACKAGE_POLICY_FIXTURE,
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
