use super::utils::{RoutingFixture, RoutingStep, check_routing};

const MULTI_PROJECT_FIXTURE: &str = r#"
//- /workspace/Cargo.toml
[workspace]
members = ["project_a", "project_b"]
resolver = "3"

//- /workspace/project_a/Cargo.toml
[package]
name = "project_a"
version = "0.1.0"
edition = "2024"

//- /workspace/project_a/src/lib.rs
pub struct ProjectA;

//- /workspace/project_a/vendor/member/Cargo.toml
[package]
name = "member"
version = "0.1.0"
edition = "2024"

//- /workspace/project_a/vendor/member/src/lib.rs
pub struct NestedMember;

//- /workspace/project_b/Cargo.toml
[package]
name = "project_b"
version = "0.1.0"
edition = "2024"

//- /workspace/project_b/src/lib.rs
pub struct ProjectB;

//- /external/thin_vec/src/lib.rs
pub struct ExternalDependency;
"#;

#[test]
fn routes_documents_to_existing_spawned_or_active_engines() {
    check_routing(
        RoutingFixture::new(MULTI_PROJECT_FIXTURE)
            .workspace_folders(["workspace"])
            .active_engine("workspace/project_a")
            .engine("workspace/project_a/vendor/member"),
        &[
            RoutingStep::workspace_action("initial active workspace action"),
            RoutingStep::open("existing project file", "workspace/project_a/src/lib.rs"),
            RoutingStep::open(
                "longest nested engine wins",
                "workspace/project_a/vendor/member/src/lib.rs",
            ),
            RoutingStep::workspace_action("workspace action follows nested engine"),
            RoutingStep::open(
                "new workspace package spawns engine",
                "workspace/project_b/src/lib.rs",
            ),
            RoutingStep::workspace_action("workspace action follows spawned engine"),
            RoutingStep::open(
                "external file falls back to active",
                "external/thin_vec/src/lib.rs",
            ),
            RoutingStep::workspace_action("external file keeps active engine"),
        ],
        r#"
initial active workspace action: active e0 /workspace/project_a
existing project file: existing e0 /workspace/project_a
longest nested engine wins: existing e1 /workspace/project_a/vendor/member
workspace action follows nested engine: active e1 /workspace/project_a/vendor/member
new workspace package spawns engine: spawn e2 /workspace/project_b
workspace action follows spawned engine: active e2 /workspace/project_b
external file falls back to active: existing e2 /workspace/project_b
external file keeps active engine: active e2 /workspace/project_b
"#,
    );
}
